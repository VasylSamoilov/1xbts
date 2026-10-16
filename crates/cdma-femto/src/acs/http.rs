use super::server::AcsState;
use super::*;

fn cookie_session_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|cookie| cookie.to_str().ok())
        .flat_map(|cookie| cookie.split(';'))
        .find_map(|kv| {
            let (name, value) = kv.trim().split_once('=')?;
            (name == SESSION_COOKIE).then(|| value.to_string())
        })
}

pub(super) fn scoped_session_key(owner: &str, session_id: &str) -> String {
    format!("{owner}\0{session_id}")
}

pub(super) async fn handle_cwmp(
    State(state): State<Arc<AcsState>>,
    axum::Extension(peer): axum::Extension<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // The digest binds to the method, always POST for CWMP.
    let request_authorization = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let (authenticated, authenticated_fap, authenticated_password) =
        state.server.authenticate_fap(request_authorization, "POST");
    // Build the RFC 2617 §3.2.3 Authentication-Info header. Strict legacy CWMP
    // clients require it to continue an authenticated session rather than restart.
    let authentication_info = if authenticated {
        request_authorization
            .zip(authenticated_password.as_deref())
            .and_then(|(header, password)| {
                cdma_cwmp::digest::authentication_info(
                    header,
                    password,
                    state.server.realm(),
                    ACS_DIGEST_NONCE,
                    ACS_DIGEST_NONCE,
                )
            })
    } else {
        None
    };
    if !authenticated {
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(
                axum::http::header::WWW_AUTHENTICATE,
                state.server.challenge(),
            )
            .header(axum::http::header::CONTENT_TYPE, "text/plain")
            // Airave 2.5.23.15 treats Content-Length: 0 as session terminate.
            .body(axum::body::Body::from("Unauthorized\n"))
            .expect("valid 401");
    }

    let legacy_airave2 = authenticated_fap
        .as_deref()
        .and_then(|feid| state.server.fap_definition(feid))
        .is_some_and(|fap| {
            matches!(
                fap.cwmp_inform_response_style,
                CwmpInformResponseStyle::LegacyAirave2
            )
        });
    let session_id = cookie_session_id(&headers).unwrap_or_else(|| {
        if legacy_airave2 {
            authenticated_fap
                .clone()
                .expect("legacy Airave2 is authenticated")
        } else {
            format!("s{}", state.session_seq.fetch_add(1, Ordering::Relaxed))
        }
    });
    let session_owner = authenticated_fap
        .as_deref()
        .map(str::to_string)
        .unwrap_or_else(|| peer.ip().to_string());
    let session_key = scoped_session_key(&session_owner, &session_id);

    let incoming_cwmp_id = if body.is_empty() {
        None
    } else {
        cdma_cwmp::parse_envelope_id(&body).ok().flatten()
    };
    let msg = if body.is_empty() {
        CwmpMessage::Empty
    } else {
        match cdma_cwmp::parse_envelope(&body) {
            Ok(m) => m,
            Err(err) => {
                log::warn!("ACS: undecodable CWMP body: {err}");
                CwmpMessage::Empty
            }
        }
    };

    if let (CwmpMessage::Inform(inform), Some(authenticated_fap)) =
        (&msg, authenticated_fap.as_deref())
        && !inform
            .device_id
            .serial_number
            .eq_ignore_ascii_case(authenticated_fap)
    {
        log::warn!("ACS: authenticated FAP {authenticated_fap} reported a different device serial");
        return Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(axum::body::Body::empty())
            .expect("valid 403");
    }

    if let CwmpMessage::Inform(inform) = &msg {
        state
            .session_faps
            .lock()
            .expect("ACS session FAP lock")
            .insert(session_key.clone(), inform.device_id.serial_number.clone());
    }
    let session_fap = state
        .session_faps
        .lock()
        .expect("ACS session FAP lock")
        .get(&session_key)
        .cloned();
    let targeted_reboot = session_fap.as_deref().is_some_and(|feid| {
        state
            .server
            .cell_state
            .as_ref()
            .is_some_and(|cells| cells.reboot_requested(feid))
    });
    let status_query_result = state
        .sessions
        .lock()
        .expect("acs sessions lock")
        .get(&session_key)
        .filter(|session| session.phase == AcsPhase::StatusQuerySent)
        .and_then(|_| match &msg {
            CwmpMessage::GetParameterValuesResponse(_) => Some(true),
            CwmpMessage::Fault(_) => Some(false),
            _ => None,
        });
    let reboot = state.server.reboot_armed() || targeted_reboot;
    if let (CwmpMessage::GetParameterValuesResponse(response), Some(feid)) =
        (&msg, session_fap.as_deref())
    {
        let password_parameter = state.server.fap_definition(feid).and_then(|definition| {
            definition
                .implementation
                .provisioning()
                .sip_password_parameter()
        });
        let has_configured_password = state
            .server
            .fap_definition(feid)
            .is_some_and(|definition| definition.site.sip_password.is_some());
        if let (Some(parameter_name), Some(passwords)) =
            (password_parameter, &state.server.learned_sip_passwords)
        {
            if !has_configured_password
                && let Some(password) = response
                    .parameters
                    .iter()
                    .find(|parameter| parameter.name == parameter_name)
                    .map(|parameter| parameter.value.as_str())
                    .filter(|password| !password.is_empty())
            {
                let mut passwords = passwords.write().expect("learned SIP password lock");
                passwords.insert(feid.to_string(), password.to_string());
                crate::sip::persist_learned_sip_passwords(&passwords);
                log::info!("ACS: learned runtime SIP credential for FAP {feid}");
            }
        }
    }
    if let Some(cell_state) = &state.server.cell_state {
        if let Some(feid) = session_fap.as_deref() {
            cell_state.observe_inner_peer(feid, peer.ip());
        }
        match (&msg, session_fap.as_deref()) {
            (CwmpMessage::Inform(inform), Some(feid)) => {
                cell_state.note_acs_inform(feid);
                if let Some(version) = inform
                    .parameter_list
                    .iter()
                    .find(|parameter| {
                        parameter.name == "InternetGatewayDevice.DeviceInfo.SoftwareVersion"
                    })
                    .map(|parameter| parameter.value.trim())
                    .filter(|version| !version.is_empty())
                {
                    cell_state.observe_software_version(feid, version.to_string());
                }
                if let Some(url) = inform
                    .parameter_list
                    .iter()
                    .find(|parameter| {
                        parameter.name
                            == "InternetGatewayDevice.ManagementServer.ConnectionRequestURL"
                    })
                    .map(|parameter| parameter.value.trim())
                    .filter(|url| !url.is_empty())
                {
                    cell_state.observe_connection_request_url(feid, url.to_string());
                }
            }
            (CwmpMessage::SetParameterValuesResponse(_), Some(feid)) => {
                cell_state.note_acs_provisioned(feid)
            }
            (CwmpMessage::GetParameterValuesResponse(response), Some(feid)) => {
                let parameters = response
                    .parameters
                    .iter()
                    .filter(|parameter| !is_sensitive_parameter(&parameter.name))
                    .map(|parameter| (parameter.name.as_str(), parameter.value.as_str()))
                    .collect::<Vec<_>>();
                if let Some(implementation) = state.server.fap_definition(feid) {
                    cell_state.update_normalized_status(
                        feid,
                        implementation.implementation.normalize_status(&parameters),
                    );
                } else {
                    cell_state.update_acs_status(feid, parameters);
                }
                if status_query_result == Some(true) {
                    cell_state.complete_status_readback(feid, true);
                }
            }
            _ => {}
        }
        if status_query_result == Some(false)
            && let Some(feid) = session_fap.as_deref()
        {
            cell_state.complete_status_readback(feid, false);
        }
    }
    if let CwmpMessage::Inform(inform) = &msg
        && let Some(sender) = &state.server.fap_route_hint_tx
    {
        let mut sip_peer = peer;
        sip_peer.set_port(5060);
        if sender
            .send(FapRouteHint {
                serial_number: inform.device_id.serial_number.clone(),
                peer: sip_peer,
            })
            .is_err()
        {
            log::warn!("ACS: SIP route hint receiver is unavailable");
        }
    }
    let radio_stage = session_fap.as_deref().and_then(|feid| {
        let enabled = state
            .server
            .cell_state
            .as_ref()?
            .pending_radio_state(feid)?;
        state
            .server
            .fap_definition(feid)?
            .implementation
            .provisioning()
            .radio_administrative_stage(enabled)
    });
    let (action, configuration_verified) = {
        let mut sessions = state.sessions.lock().expect("acs sessions lock");
        let session = sessions
            .entry(session_key.clone())
            .or_insert_with(AcsSession::new);
        if let Some(implementation) = session_fap
            .as_deref()
            .and_then(|feid| state.server.fap_definition(feid))
        {
            session.set_implementation(implementation.implementation.clone());
            session.set_fap_context(
                &implementation,
                state
                    .server
                    .term_auth_psk
                    .read()
                    .expect("term auth psk lock")
                    .clone(),
            );
        }
        let configured_sip_password = session_fap.as_deref().and_then(|feid| {
            state
                .server
                .fap_definition(feid)
                .and_then(|definition| definition.site.sip_password.clone())
        });
        if let (Some(feid), Some(password), Some(passwords)) = (
            session_fap.as_deref(),
            configured_sip_password.as_ref(),
            &state.server.learned_sip_passwords,
        ) {
            let mut store = passwords.write().expect("learned SIP password lock");
            if store.get(feid).map(String::as_str) != Some(password.as_str()) {
                store.insert(feid.to_string(), password.clone());
                crate::sip::persist_learned_sip_passwords(&store);
                log::info!("ACS: using configured SIP credential for FAP {feid}");
            }
        }
        session.set_read_sip_password(
            state.server.learned_sip_passwords.is_some() && configured_sip_password.is_none(),
        );
        let enabled = state.server.service_enabled(true);
        let program = state.server.program(session_fap.as_deref());
        session.set_reapply_stages(&program.reapply_stages);
        session.set_batch_queries(program.batch_queries);
        session.set_feid(session_fap.clone());
        let stages = radio_stage
            .as_ref()
            .map_or(program.stages.as_slice(), std::slice::from_ref);
        let action = session.step_with_objects(
            &msg,
            &program.ensure_objects,
            stages,
            &program.discovery_query,
            &program.status_query,
            reboot,
            enabled,
        );
        (action, session.configuration_verified())
    };
    if let CwmpMessage::Fault(fault) = &msg {
        log::info!(
            "ACS: [{}] <- CPE Fault {} {}",
            session_fap.as_deref().unwrap_or("?"),
            fault.fault_code,
            fault.fault_string
        );
    }
    log_outgoing(session_fap.as_deref(), &action);
    if configuration_verified {
        if let (Some(cell_state), Some(feid)) = (&state.server.cell_state, session_fap.as_deref()) {
            cell_state.note_acs_provisioned(feid);
            if radio_stage.is_some() {
                cell_state.complete_radio_state(feid);
            }
        }
    }
    if let AcsAction::Respond(m) = &action {
        if matches!(**m, CwmpMessage::Reboot(_)) {
            if targeted_reboot {
                if let (Some(cell_state), Some(feid)) =
                    (&state.server.cell_state, session_fap.as_deref())
                {
                    cell_state.complete_reboot(feid);
                }
            } else {
                state.server.disarm_reboot();
            }
        }
    }

    let cookie = format!("{SESSION_COOKIE}={session_id}");
    match action {
        AcsAction::Respond(msg) => {
            let response_id = if matches!(msg.as_ref(), CwmpMessage::InformResponse(_)) {
                incoming_cwmp_id.as_deref().unwrap_or(&session_id)
            } else {
                &session_id
            };
            let inform_response_style = session_fap
                .as_deref()
                .and_then(|feid| state.server.fap_definition(feid))
                .map_or(CwmpInformResponseStyle::Standard, |fap| {
                    fap.cwmp_inform_response_style
                });
            let mut xml = match (msg.as_ref(), inform_response_style) {
                (CwmpMessage::InformResponse(response), CwmpInformResponseStyle::LegacyAirave2) => {
                    cdma_cwmp::write_legacy_inform_response(response, response_id)
                }
                _ => cdma_cwmp::write_envelope(&msg, response_id).unwrap_or_default(),
            };
            // Airvana tragent overwrites the final response byte with NUL.
            // Pad SOAP with a newline to preserve the closing envelope tag.
            if !xml.is_empty() {
                xml.push('\n');
            }
            let legacy_inform_response =
                matches!(
                    inform_response_style,
                    CwmpInformResponseStyle::LegacyAirave2
                ) && matches!(msg.as_ref(), CwmpMessage::InformResponse(_));
            let content_type = if legacy_inform_response {
                "text/xml"
            } else {
                "text/xml; charset=utf-8"
            };
            let mut response = Response::builder()
                .status(StatusCode::OK)
                .header(axum::http::header::CONTENT_TYPE, content_type);
            response = response.header(axum::http::header::SET_COOKIE, cookie);
            if let Some(info) = &authentication_info {
                response = response.header("authentication-info", info);
            }
            response
                .body(axum::body::Body::from(xml))
                .expect("valid 200")
        }
        AcsAction::EndSession => {
            state
                .session_faps
                .lock()
                .expect("ACS session FAP lock")
                .remove(&session_key);
            state
                .sessions
                .lock()
                .expect("acs sessions lock")
                .remove(&session_key);
            let mut response = Response::builder().status(StatusCode::OK);
            if let Some(info) = &authentication_info {
                response = response.header("authentication-info", info);
            }
            response
                .body(axum::body::Body::empty())
                .expect("valid empty 200")
        }
    }
}
