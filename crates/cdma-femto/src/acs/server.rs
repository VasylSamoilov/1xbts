use super::http::handle_cwmp;
use super::transport::LegacyHeaderCaseIo;
use super::*;

pub struct AcsServer {
    realm: String,
    require_site_approval: bool,
    credentials: Arc<std::sync::RwLock<Vec<CpeCredential>>>,
    reboot_armed: std::sync::atomic::AtomicBool,
    pub(super) cell_state: Option<Arc<FemtoCellState>>,
    fap_registry: Option<SharedFapRegistry>,
    pub(super) fap_route_hint_tx: Option<tokio::sync::mpsc::UnboundedSender<FapRouteHint>>,
    pub(super) learned_sip_passwords: Option<Arc<std::sync::RwLock<HashMap<String, String>>>>,
    pub(super) a11_provisioning: Arc<std::sync::RwLock<Option<crate::config::A11Provisioning>>>,
    pub(super) term_auth_psk: Arc<std::sync::RwLock<Option<String>>>,
}

pub(super) struct AcsProgram {
    pub(super) ensure_objects: Vec<CwmpObject>,
    pub(super) stages: Vec<ProvisioningStage>,
    pub(super) discovery_query: Vec<CwmpObject>,
    pub(super) status_query: Vec<String>,
    pub(super) reapply_stages: Vec<String>,
    pub(super) batch_queries: bool,
}

impl AcsProgram {
    fn empty() -> Self {
        Self {
            ensure_objects: Vec::new(),
            stages: Vec::new(),
            discovery_query: Vec::new(),
            status_query: Vec::new(),
            reapply_stages: Vec::new(),
            batch_queries: true,
        }
    }
}

impl AcsServer {
    pub fn new(config: &AcsConfig) -> Self {
        Self::build(config, None)
    }

    pub fn with_cell_state(config: &AcsConfig, cell_state: Arc<FemtoCellState>) -> Self {
        Self::build(config, Some(cell_state))
    }

    fn build(config: &AcsConfig, cell_state: Option<Arc<FemtoCellState>>) -> Self {
        Self {
            realm: config.realm.clone(),
            require_site_approval: config.require_site_approval,
            credentials: Arc::new(std::sync::RwLock::new(config.cpe_credentials.clone())),
            reboot_armed: std::sync::atomic::AtomicBool::new(config.reboot),
            cell_state,
            fap_registry: None,
            fap_route_hint_tx: None,
            learned_sip_passwords: None,
            a11_provisioning: Arc::new(std::sync::RwLock::new(None)),
            term_auth_psk: Arc::new(std::sync::RwLock::new(None)),
        }
    }

    pub fn with_a11_provisioning(self, value: Option<crate::config::A11Provisioning>) -> Self {
        *self
            .a11_provisioning
            .write()
            .expect("A11 provisioning lock") = value;
        self
    }

    pub fn a11_provisioning_handle(
        &self,
    ) -> Arc<std::sync::RwLock<Option<crate::config::A11Provisioning>>> {
        self.a11_provisioning.clone()
    }

    pub fn with_term_auth_psk(self, psk: Option<String>) -> Self {
        *self.term_auth_psk.write().expect("term auth psk lock") = psk;
        self
    }

    pub fn credentials_handle(&self) -> Arc<std::sync::RwLock<Vec<CpeCredential>>> {
        self.credentials.clone()
    }

    pub fn term_auth_psk_handle(&self) -> Arc<std::sync::RwLock<Option<String>>> {
        self.term_auth_psk.clone()
    }

    pub fn with_fap_route_hints(
        mut self,
        sender: tokio::sync::mpsc::UnboundedSender<FapRouteHint>,
    ) -> Self {
        self.fap_route_hint_tx = Some(sender);
        self
    }

    pub fn with_fap_registry(mut self, registry: SharedFapRegistry) -> Self {
        self.fap_registry = Some(registry);
        self
    }

    pub fn fap_registry_handle(&self) -> Option<SharedFapRegistry> {
        self.fap_registry.clone()
    }

    pub(super) fn fap_definition(&self, feid: &str) -> Option<crate::fap::FapRuntimeDefinition> {
        self.fap_registry
            .as_ref()?
            .read()
            .expect("fap registry lock")
            .get(feid)
            .cloned()
    }

    pub fn with_learned_sip_passwords(
        mut self,
        passwords: Arc<std::sync::RwLock<HashMap<String, String>>>,
    ) -> Self {
        self.learned_sip_passwords = Some(passwords);
        self
    }

    pub fn reboot_armed(&self) -> bool {
        self.reboot_armed.load(Ordering::SeqCst)
    }

    pub fn disarm_reboot(&self) {
        self.reboot_armed.store(false, Ordering::SeqCst);
    }

    pub fn realm(&self) -> &str {
        &self.realm
    }

    pub(super) fn program(&self, feid: Option<&str>) -> AcsProgram {
        let (Some(feid), Some(registry)) = (feid, self.fap_registry.as_ref()) else {
            return AcsProgram::empty();
        };
        let registry = registry.read().expect("fap registry lock").clone();
        match registry
            .get(feid)
            .and_then(|definition| definition.acs.as_deref())
        {
            Some(program) => AcsProgram {
                ensure_objects: program.ensure_objects.clone(),
                stages: program.stages.clone(),
                discovery_query: program.discovery_query.clone(),
                status_query: program.status_query.clone(),
                reapply_stages: program.reapply_stages.clone(),
                batch_queries: program.batch_queries.unwrap_or(true),
            },
            None => AcsProgram::empty(),
        }
    }

    pub fn service_enabled(&self, site_approved: bool) -> bool {
        !self.require_site_approval || site_approved
    }

    pub fn authenticate(&self, authorization: Option<&str>, method: &str) -> bool {
        self.authenticate_fap(authorization, method).0
    }

    pub(super) fn authenticate_fap(
        &self,
        authorization: Option<&str>,
        method: &str,
    ) -> (bool, Option<String>, Option<String>) {
        let Some(auth) = authorization else {
            return (false, None, None);
        };
        self.credentials
            .read()
            .expect("acs credentials lock")
            .iter()
            .find(|cred| {
                cdma_cwmp::digest::verify_digest(
                    auth,
                    &cred.username,
                    &cred.password,
                    &self.realm,
                    ACS_DIGEST_NONCE,
                    method,
                )
            })
            .map(|credential| {
                (
                    true,
                    credential.feid.clone(),
                    Some(credential.password.clone()),
                )
            })
            .unwrap_or((false, None, None))
    }

    pub fn challenge(&self) -> String {
        format!(
            "Digest realm=\"{}\", qop=\"auth\", nonce=\"{}\"",
            self.realm, ACS_DIGEST_NONCE
        )
    }

    pub async fn serve(self, listen: SocketAddr) -> std::io::Result<()> {
        let state = Arc::new(AcsState {
            server: self,
            sessions: Mutex::new(HashMap::new()),
            session_faps: Mutex::new(HashMap::new()),
            session_seq: AtomicU64::new(1),
        });
        let app = Router::new().fallback(any(handle_cwmp)).with_state(state);
        let listener = tokio::net::TcpListener::bind(listen).await?;
        log::info!("ACS (TR-069) listening on {listen}");
        loop {
            let (stream, peer) = listener.accept().await?;
            let app = app.clone();
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |mut request| {
                    request.extensions_mut().insert(peer);
                    app.clone().call(request)
                });
                if let Err(error) = http1::Builder::new()
                    .title_case_headers(true)
                    .serve_connection(TokioIo::new(LegacyHeaderCaseIo(stream)), service)
                    .await
                {
                    log::debug!("ACS HTTP connection from {peer} ended: {error}");
                }
            });
        }
    }
}

pub(super) struct AcsState {
    pub(super) server: AcsServer,
    pub(super) sessions: Mutex<HashMap<String, AcsSession>>,
    pub(super) session_faps: Mutex<HashMap<String, String>>,
    pub(super) session_seq: AtomicU64,
}
