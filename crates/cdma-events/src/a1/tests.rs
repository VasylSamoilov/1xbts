use super::*;
use crate::proto::network_event;

// The X.S0059-200 Table 2 reference FemtoInterfaceMsg body: Location Updating
// Request for IMSI_S 9135198800, eleven information elements.
const REFERENCE_FEMTO: &[u8] = b"08 (=\"0 Location Updating Request\")\r\n\
0D=DQaWMRWJCPA=\r\n\
12=EgzJAOYAAAAAAQMBAAY=\r\n\
1F=HwE=\r\n\
0D=DQUFKa4zmQ==\r\n\
35=NQE=\r\n\
42=QgQBATjd\r\n\
28=KP0=\r\n\
40=QAA=\r\n\
41=QQUBAAAA/Q==\r\n\
0D=DQjxAAAAAAAA8A==\r\n";

fn reference_body() -> FemtoInterfaceMsg {
    FemtoInterfaceMsg::parse(REFERENCE_FEMTO).expect("reference body parses")
}

fn ie(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
        .collect()
}

fn decode(hex: &str) -> proto::a1_information_element::Decoded {
    let bytes = ie(hex);
    decode_with(bytes[0], hex)
}

fn decode_with(iei: u8, hex: &str) -> proto::a1_information_element::Decoded {
    decode_ie(iei, &ie(hex)).expect("decodes")
}

#[test]
fn decodes_tlv_framed_elements() {
    use proto::a1_information_element::Decoded;
    // Mobile Identity: IEI, length, then the value.
    match decode("0D0505803AF4C0") {
        Decoded::MobileIdentity(id) => {
            assert_eq!(id.form, proto::A1MobileIdentityForm::Esn as i32);
            assert_eq!(id.esn, 0x803A_F4C0);
        }
        other => panic!("expected mobile identity, got {other:?}"),
    }
    match decode("0B03010805") {
        Decoded::ChannelType(channel) => {
            assert_eq!(channel.speech_or_data_indicator, 0x01);
            assert_eq!(channel.channel_rate_and_type, 0x08);
            assert_eq!(channel.coding, 0x05);
        }
        other => panic!("expected channel type, got {other:?}"),
    }
}

#[test]
fn decodes_fixed_width_elements_without_a_length_octet() {
    use proto::a1_information_element::Decoded;
    // `1F00` is two bytes: the value is the whole of what follows the IEI.
    // Reading a length octet here would consume the value itself.
    match decode("1F00") {
        Decoded::RegistrationType(reg) => {
            assert_eq!(reg.r#type, proto::A1RegistrationType::TimerBased as i32)
        }
        other => panic!("expected registration type, got {other:?}"),
    }
    match decode("030003") {
        Decoded::ServiceOption(so) => {
            assert_eq!(so.service_option, proto::ServiceOption::Evrc as i32)
        }
        other => panic!("expected service option, got {other:?}"),
    }
    match decode("3502") {
        Decoded::SlotCycleIndex(sci) => assert_eq!(sci.value, 2),
        other => panic!("expected slot cycle index, got {other:?}"),
    }
    match decode("330000002A") {
        Decoded::Tag(tag) => assert_eq!(tag.value, 42),
        other => panic!("expected tag, got {other:?}"),
    }
    match decode("230000") {
        Decoded::ChannelNumber(channel) => assert_eq!(channel.value, 0),
        other => panic!("expected channel number, got {other:?}"),
    }
}

#[test]
fn decodes_the_type_1_cm_service_type() {
    use proto::a1_information_element::Decoded;
    // A Type 1 element packs the IEI and the value into one octet, so
    // there is no value slice to take at all.
    match decode_with(IE_CM_SERVICE_TYPE, "94") {
        Decoded::CmServiceType(service) => assert_eq!(
            service.r#type,
            proto::A1CmServiceType::ShortMessageTransfer as i32
        ),
        other => panic!("expected CM service type, got {other:?}"),
    }
    match decode_with(IE_CM_SERVICE_TYPE, "91") {
        Decoded::CmServiceType(service) => assert_eq!(
            service.r#type,
            proto::A1CmServiceType::MobileOriginatingCall as i32
        ),
        other => panic!("expected CM service type, got {other:?}"),
    }
}

#[test]
fn decodes_called_party_digits() {
    use proto::a1_information_element::Decoded;
    match decode("5E03809999") {
        Decoded::CalledParty(called) => {
            assert_eq!(called.digits, "9999");
            assert_eq!(called.number_type, proto::A1NumberType::Unknown as i32);
        }
        other => panic!("expected called party, got {other:?}"),
    }
    // 0xF is the filler that ends an odd-length digit string.
    match decode("5E0380F119") {
        Decoded::CalledParty(called) => assert_eq!(called.digits, "1"),
        other => panic!("expected called party, got {other:?}"),
    }
}

#[test]
fn authentication_parameters_expose_the_value_not_a_guessed_field_split() {
    use proto::a1_information_element::Decoded;
    match decode("420401002BC1") {
        Decoded::AuthResponse(auth) => {
            assert_eq!(auth.value_hex, "01002BC1");
            assert_eq!(auth.value, vec![0x01, 0x00, 0x2B, 0xC1]);
        }
        other => panic!("expected auth response, got {other:?}"),
    }
    match decode("41050144505213") {
        Decoded::AuthChallenge(auth) => assert_eq!(auth.value_hex, "0144505213"),
        other => panic!("expected auth challenge, got {other:?}"),
    }
    match decode("2844") {
        Decoded::AuthConfirmation(auth) => assert_eq!(auth.randc, 0x44),
        other => panic!("expected auth confirmation, got {other:?}"),
    }
    match decode("4000") {
        Decoded::AuthCount(auth) => assert_eq!(auth.count, 0),
        other => panic!("expected auth count, got {other:?}"),
    }
}

#[test]
fn an_unmodelled_element_decodes_to_nothing_but_keeps_its_bytes() {
    let classmark = ie("120CC80062000201BA0103010006");
    assert!(decode_ie(classmark[0], &classmark).is_none());

    let event = a1_signaling_event(&reference_body(), &A1Context::default());
    for element in &event.information_elements {
        let expected: Vec<u8> = event
            .information_elements
            .iter()
            .find(|candidate| candidate.iei == element.iei)
            .map(|candidate| candidate.content.clone())
            .expect("element");
        assert!(!expected.is_empty(), "raw bytes are kept regardless");
    }
}

#[test]
fn a_truncated_element_does_not_panic() {
    for truncated in ["0D", "0D06", "0B0301", "5E03", "42", "1F", "03"] {
        let bytes = ie(truncated);
        let _ = decode_ie(bytes[0], &bytes);
    }
}

#[test]
fn correlated_imsi_gives_an_identity_to_a_message_that_carries_none() {
    let mut accept = reference_body();
    accept.ies.clear();
    assert!(mobile_identity(&A1Body::from_fx2(&accept).information_elements, None).is_none());

    let event = a1_event(
        EventSource::Femto,
        "FEID-TEST",
        &accept,
        &A1Context {
            direction: proto::A1Direction::Downlink,
            correlated_imsi: Some("310009135198800".to_string()),
            ..A1Context::default()
        },
    );
    let identity = event.identity.expect("correlated identity");
    assert_eq!(identity.imsi, "310009135198800");
}

#[test]
fn correlated_imsi_promotes_imsi_s_to_the_full_imsi() {
    let message = reference_body();
    let short = mobile_identity(&A1Body::from_fx2(&message).information_elements, None)
        .expect("body identity");
    assert_eq!(short.imsi, "9135198800");

    let event = a1_event(
        EventSource::Femto,
        "FEID-TEST",
        &message,
        &A1Context {
            correlated_imsi: Some("310009135198800".to_string()),
            ..A1Context::default()
        },
    );
    let identity = event.identity.expect("identity");
    assert_eq!(identity.imsi, "310009135198800");
    assert_eq!(identity.esn, short.esn);
}

#[test]
fn a_correlated_imsi_for_a_different_mobile_does_not_overwrite_the_body() {
    let message = reference_body();
    let event = a1_signaling_event(
        &message,
        &A1Context {
            correlated_imsi: Some("310004440000000".to_string()),
            ..A1Context::default()
        },
    );
    assert_eq!(
        event.mobile_identity.expect("identity").imsi,
        "9135198800",
        "body IMSI_S must win over an unrelated correlated IMSI"
    );
}

#[test]
fn a1_event_names_and_classifies_the_message() {
    let event = a1_signaling_event(&reference_body(), &A1Context::default());

    assert_eq!(event.message_type_value, 0x08);
    assert_eq!(event.message_type_name, "Location Updating Request");
    assert_eq!(
        event.message_type,
        proto::A1MessageType::LocationUpdatingRequest as i32
    );
    assert_eq!(
        event.discriminator,
        proto::A1MessageDiscriminator::Dtap as i32
    );
}

#[test]
fn a1_event_carries_the_complete_sdu_and_every_ie() {
    let message = reference_body();
    let event = a1_signaling_event(&message, &A1Context::default());

    // DTAP Location Updating Request: PD=0x05 Mobility Management,
    // reserved octet, then the message type.
    assert_eq!(&event.sdu[..3], &[0x05, 0x00, 0x08]);
    let ie_bytes: usize = message.ies.iter().map(|ie| ie.content.len()).sum();
    assert_eq!(event.sdu.len(), 3 + ie_bytes);
    assert_eq!(event.sdu_hex, hex_upper(&event.sdu));

    assert_eq!(event.information_elements.len(), message.ies.len());
    assert_eq!(event.information_elements[0].iei, 0x0d);
    assert_eq!(event.information_elements[0].name, "Mobile Identity");
    assert_eq!(
        event.information_elements[1].name,
        "Classmark Information Type 2"
    );
}

#[test]
fn a1_event_extracts_every_mobile_identity_form() {
    let event = a1_signaling_event(&reference_body(), &A1Context::default());
    let identity = event.mobile_identity.expect("identity present");

    assert_eq!(identity.imsi, "9135198800");
    assert_eq!(identity.esn, 0x29AE_3399);
    assert!(!identity.meid.is_empty());
}

#[test]
fn a1_event_prefers_the_correlated_full_imsi() {
    let context = A1Context {
        correlated_imsi: Some("310009135198800".to_string()),
        ..A1Context::default()
    };
    let event = a1_signaling_event(&reference_body(), &context);

    assert_eq!(
        event.mobile_identity.expect("identity present").imsi,
        "310009135198800"
    );
}

#[test]
fn a1_event_publishes_identity_for_hlr_enrichment() {
    let event = a1_event(
        EventSource::Femto,
        "FEID-TEST",
        &reference_body(),
        &A1Context::default(),
    );

    let identity = event.identity.expect("bus identity set");
    assert_eq!(identity.imsi, "9135198800");
    assert_eq!(identity.esn, 0x29AE_3399);
}

#[test]
fn a1_events_sit_at_the_top_level_not_inside_the_femto_wrapper() {
    let event = a1_event(
        EventSource::Femto,
        "FEID-TEST",
        &reference_body(),
        &A1Context::default(),
    );

    assert_eq!(event.source, EventSource::Femto as i32);
    match event.body {
        Some(network_event::Body::A1(a1)) => assert_eq!(a1.peer_id, "FEID-TEST"),
        other => panic!("expected top-level A1 body, got {other:?}"),
    }
}

#[test]
fn bsmap_messages_carry_their_own_framing() {
    let mut message = FemtoInterfaceMsg::new(
        MSG_TYPE_ADDS_TRANSFER,
        vec![
            cdma_ios::femto_interface_msg::FemtoIe::tlv(0x0d, &[0x06, 0x91, 0x35]).unwrap(),
            // ADDS User Part: SMS burst type followed by the C.S0015 payload.
            cdma_ios::femto_interface_msg::FemtoIe::tlv(0x3d, &[0x03, 0xaa, 0xbb, 0xcc]).unwrap(),
        ],
    );
    message.discriminator = Some(Discriminator::Bsmap);

    let event = a1_signaling_event(&message, &A1Context::default());

    assert_eq!(event.message_type_name, "ADDS Transfer");
    // BSMAP framing: discrimination octet, length, message type.
    assert_eq!(event.sdu[0], 0x00);
    assert_eq!(event.sdu[2], MSG_TYPE_ADDS_TRANSFER);
    let ie_bytes: usize = message.ies.iter().map(|ie| ie.content.len()).sum();
    assert_eq!(event.sdu.len(), 3 + ie_bytes);
    assert_eq!(usize::from(event.sdu[1]), 1 + ie_bytes);

    let adds = event.adds.expect("ADDS payload decoded");
    assert_eq!(adds.burst_type, proto::AddsBurstType::Sms as i32);
    assert_eq!(adds.payload_length_bytes, 3);
    assert!(event.summary.contains("SMS 3 bytes"));
}

#[test]
fn unmodelled_message_types_keep_their_sdu() {
    let message = FemtoInterfaceMsg::new(
        0x7f,
        vec![cdma_ios::femto_interface_msg::FemtoIe::tlv(0x77, &[1, 2, 3]).unwrap()],
    );
    let event = a1_signaling_event(&message, &A1Context::default());

    assert_eq!(event.message_type, proto::A1MessageType::Other as i32);
    assert_eq!(event.message_type_name, "A1 0x7F");
    assert_eq!(event.information_elements.len(), 1);
    assert_eq!(event.information_elements[0].name, "");
    assert_eq!(
        event.information_elements[0].content,
        vec![0x77, 3, 1, 2, 3]
    );
    assert_eq!(event.sdu, vec![0x03, 0x00, 0x7f, 0x77, 3, 1, 2, 3]);
}

#[test]
fn native_complete_layer_three_keeps_wire_bytes_and_identifies_the_mobile() {
    let mut request = reference_body().to_location_updating_request().unwrap();
    request.mobile_identity_imsi = MobileIdentity::Imsi("310009135198800".into());
    request.mobile_identity_meid = Some(MobileIdentity::Meid([0xa0, 0, 0, 0x12, 0xab, 0x34, 0xcd]));
    let complete = cdma_ios::CompleteLayer3InformationMessage {
        cell_identifier: CellId {
            cell: 413,
            sector: 0,
        },
        layer3_information: cdma_ios::Layer3Information::from_location_updating_request(&request)
            .unwrap(),
    };
    let payload = complete.encode().unwrap();
    let encoded = cdma_ios::EncodedA1Message::from_message_for_call(
        &cdma_ios::Message::new(
            cdma_ios::MessageType::CompleteLayer3Information,
            payload.clone(),
        ),
        Some(77),
    );
    let event = a1_event_from_wire(
        EventSource::Bsc,
        "bsc-1",
        &encoded,
        &A1Context {
            direction: proto::A1Direction::Uplink,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(event.source, EventSource::Bsc as i32);
    let identity = event.identity.unwrap();
    assert_eq!(identity.imsi, "310009135198800");
    assert_eq!(identity.meid, "a0000012ab34cd");
    let Some(network_event::Body::A1(a1)) = event.body else {
        panic!("expected A1");
    };
    assert_eq!(
        a1.message_type,
        proto::A1MessageType::CompleteLayer3Information as i32
    );
    assert_eq!(a1.sdu, payload);
    assert_eq!(a1.a1_call_id, 77);
    assert_eq!(a1.peer_id, "bsc-1");
    assert_eq!(a1.cell.unwrap().cell, 413);
}

#[test]
fn native_dtap_uses_wire_type_instead_of_the_envelope_routing_tag() {
    let payload = cdma_ios::LocationUpdatingAcceptMessage {
        location_area_identification: None,
    }
    .encode()
    .unwrap();
    let encoded = cdma_ios::EncodedA1Message::from_message(&cdma_ios::Message::new(
        cdma_ios::MessageType::LocationUpdatingAccept,
        payload.clone(),
    ));
    let event =
        a1_event_from_wire(EventSource::Bsc, "bsc-1", &encoded, &A1Context::default()).unwrap();
    let Some(network_event::Body::A1(a1)) = event.body else {
        panic!("expected A1");
    };
    assert_eq!(a1.discriminator, proto::A1MessageDiscriminator::Dtap as i32);
    assert_eq!(
        a1.message_type,
        proto::A1MessageType::LocationUpdatingAccept as i32
    );
    assert_eq!(a1.sdu, cdma_ios::split_a1_frame(&payload).unwrap().sdu);
}
