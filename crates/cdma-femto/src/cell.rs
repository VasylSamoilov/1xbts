use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use cdma_ios::MobileIdentity;

use crate::config::FemtoConfig;
use crate::fap::NormalizedFapStatus;

#[derive(Debug, Clone)]
pub struct ConnectionRequestCredentials {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone)]
pub struct FemtoMobileSnapshot {
    pub feid: String,
    pub imsi: String,
    pub esn: Option<u32>,
    pub meid: Option<String>,
    pub state: String,
    pub last_seen_ms: u64,
}

#[derive(Debug, Clone)]
pub struct FemtoCellSnapshot {
    pub feid: String,
    pub profile: String,
    pub cell_id: u16,
    pub sector: u8,
    pub sid: u32,
    pub nid: u32,
    pub band_class: u32,
    pub channel: u32,
    pub acs_configured: bool,
    pub acs_provisioned: bool,
    pub sip_public_identity: Option<String>,
    pub sip_peer: Option<SocketAddr>,
    pub inner_peer_address: Option<IpAddr>,
    /// The `ConnectionRequestURL` the CPE last advertised in an Inform. Older
    /// (2.x) firmware serves its connection request here (authenticated),
    /// rather than on the fixed `GET /acscall` path 5.x uses.
    pub connection_request_url: Option<String>,
    pub connection_request_credentials: Option<ConnectionRequestCredentials>,
    pub gps_status: Option<String>,
    pub timing_status: Option<String>,
    pub gps_latitude: Option<f64>,
    pub gps_longitude: Option<f64>,
    pub software_version: Option<String>,
    pub location_validation_status: Option<String>,
    pub radio_operational_state: Option<String>,
    pub last_acs_inform_ms: Option<u64>,
    pub last_updated_ms: u64,
}

#[derive(Debug, Clone)]
struct ConfiguredFap {
    feid: String,
    profile: String,
    cell_id: u16,
    sector: u8,
    sid: u32,
    nid: u32,
    band_class: u32,
    channel: u32,
    acs_configured: bool,
    connection_request_credentials: Option<ConnectionRequestCredentials>,
}

#[derive(Debug, Default)]
struct RuntimeFap {
    sip_public_identity: Option<String>,
    sip_peer: Option<SocketAddr>,
    inner_peer_address: Option<IpAddr>,
    connection_request_url: Option<String>,
    acs_provisioned: bool,
    gps_status: Option<String>,
    timing_status: Option<String>,
    gps_latitude: Option<f64>,
    gps_longitude: Option<f64>,
    software_version: Option<String>,
    location_validation_status: Option<String>,
    radio_operational_state: Option<String>,
    last_acs_inform_ms: Option<u64>,
    status_readback_generation: u64,
    status_readback_succeeded: bool,
    mobiles: HashMap<String, FemtoMobileSnapshot>,
    last_updated_ms: u64,
    pending_radio_enabled: Option<bool>,
    reboot_requested: bool,
}

#[derive(Debug)]
pub struct FemtoCellState {
    configured: RwLock<HashMap<String, ConfiguredFap>>,
    runtime: Mutex<HashMap<String, RuntimeFap>>,
    peer_state_path: Option<PathBuf>,
}

impl FemtoCellState {
    fn build_configured(config: &FemtoConfig) -> HashMap<String, ConfiguredFap> {
        config
            .faps
            .iter()
            .filter_map(|(feid, definition)| {
                let site = config.flat_sites.get(&definition.site)?;
                let connection_request_credentials = config
                    .fap_profiles
                    .get(&definition.profile)
                    .and_then(|profile| profile.connection_request.as_ref())
                    .map(|creds| ConnectionRequestCredentials {
                        username: creds.username.clone(),
                        password: creds.password.clone(),
                    });
                Some((
                    feid.clone(),
                    ConfiguredFap {
                        feid: feid.clone(),
                        profile: definition.profile.clone(),
                        cell_id: site.cell_id,
                        sector: site.sector,
                        sid: site.sid,
                        nid: site.nid,
                        band_class: site.band_class,
                        channel: site.cdma_channel,
                        acs_configured: config
                            .acs
                            .cpe_credentials
                            .iter()
                            .any(|credential| credential.feid.as_deref() == Some(feid)),
                        connection_request_credentials,
                    },
                ))
            })
            .collect::<HashMap<_, _>>()
    }

    pub fn reload_from_config(&self, config: &FemtoConfig) {
        let configured = Self::build_configured(config);
        {
            let mut runtime = self.runtime.lock().expect("femto cell state lock");
            for feid in configured.keys() {
                runtime.entry(feid.clone()).or_default();
            }
        }
        *self.configured.write().expect("femto cell state lock") = configured;
    }

    pub fn from_config(config: &FemtoConfig) -> Arc<Self> {
        let configured = Self::build_configured(config);
        let peer_state_path = std::env::var_os("FEMTO_STATE_DIR")
            .map(PathBuf::from)
            .map(|directory| directory.join("management-peers.json"));
        let learned_peers = peer_state_path
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|contents| serde_json::from_slice::<HashMap<String, IpAddr>>(&contents).ok())
            .unwrap_or_default();
        let runtime = configured
            .keys()
            .map(|feid| {
                let runtime = RuntimeFap {
                    inner_peer_address: learned_peers.get(feid).copied(),
                    ..RuntimeFap::default()
                };
                (feid.clone(), runtime)
            })
            .collect();
        Arc::new(Self {
            configured: RwLock::new(configured),
            runtime: Mutex::new(runtime),
            peer_state_path,
        })
    }

    pub fn untracked() -> Arc<Self> {
        let configured = ConfiguredFap {
            feid: "test".to_string(),
            profile: "standard-xs0059".to_string(),
            cell_id: 1,
            sector: 0,
            sid: 1,
            nid: 1,
            band_class: 0,
            channel: 1,
            acs_configured: false,
            connection_request_credentials: None,
        };
        Arc::new(Self {
            configured: RwLock::new(HashMap::from([(configured.feid.clone(), configured)])),
            runtime: Mutex::new(HashMap::from([("test".to_string(), RuntimeFap::default())])),
            peer_state_path: None,
        })
    }

    pub fn register_fap(&self, feid: &str, public_identity: String, peer: SocketAddr) {
        if let Some(runtime) = self
            .runtime
            .lock()
            .expect("femto cell state lock")
            .get_mut(feid)
        {
            runtime.sip_public_identity = Some(public_identity);
            runtime.sip_peer = Some(peer);
            runtime.inner_peer_address = Some(peer.ip());
            runtime.last_updated_ms = now_ms();
        }
        self.persist_peers();
    }

    pub fn observe_inner_peer(&self, feid: &str, peer: IpAddr) {
        if let Some(runtime) = self
            .runtime
            .lock()
            .expect("femto cell state lock")
            .get_mut(feid)
        {
            runtime.inner_peer_address = Some(peer);
            runtime.last_updated_ms = now_ms();
        }
        self.persist_peers();
    }

    pub fn feid_for_inner_peer(&self, peer: IpAddr) -> Option<String> {
        self.runtime
            .lock()
            .expect("femto cell state lock")
            .iter()
            .find_map(|(feid, runtime)| {
                (runtime.inner_peer_address == Some(peer)).then(|| feid.clone())
            })
    }

    fn persist_peers(&self) {
        let Some(path) = &self.peer_state_path else {
            return;
        };
        let peers = self
            .runtime
            .lock()
            .expect("femto cell state lock")
            .iter()
            .filter_map(|(feid, runtime)| {
                runtime.inner_peer_address.map(|peer| (feid.clone(), peer))
            })
            .collect::<HashMap<_, _>>();
        let Some(parent) = path.parent() else {
            return;
        };
        if let Err(error) = std::fs::create_dir_all(parent)
            .and_then(|()| serde_json::to_vec(&peers).map_err(std::io::Error::other))
            .and_then(|contents| std::fs::write(path, contents))
        {
            log::warn!("cannot persist femtocell management peers: {error}");
        }
    }

    pub fn deregister_fap(&self, feid: &str) {
        if let Some(runtime) = self
            .runtime
            .lock()
            .expect("femto cell state lock")
            .get_mut(feid)
        {
            runtime.sip_public_identity = None;
            runtime.sip_peer = None;
            runtime.last_updated_ms = now_ms();
        }
    }

    pub fn observe_mobile(
        &self,
        feid: &str,
        imsi: &str,
        esn: Option<&MobileIdentity>,
        meid: Option<&MobileIdentity>,
        state: &str,
    ) {
        let now = now_ms();
        let observed_esn = esn.and_then(esn_value);
        let observed_meid = meid.and_then(meid_value);
        if let Some(runtime) = self
            .runtime
            .lock()
            .expect("femto cell state lock")
            .get_mut(feid)
        {
            let snapshot =
                runtime
                    .mobiles
                    .entry(imsi.to_string())
                    .or_insert_with(|| FemtoMobileSnapshot {
                        feid: feid.to_string(),
                        imsi: imsi.to_string(),
                        esn: None,
                        meid: None,
                        state: state.to_string(),
                        last_seen_ms: now,
                    });
            // A later observation carrying only the IMSI (an SMS ack, an MO SMS
            // before CM identity details arrive) must not erase an ESN or MEID we
            // already learned. Keep the last-known value when the new one is absent.
            if observed_esn.is_some() {
                snapshot.esn = observed_esn;
            }
            if observed_meid.is_some() {
                snapshot.meid = observed_meid;
            }
            snapshot.feid = feid.to_string();
            snapshot.imsi = imsi.to_string();
            snapshot.state = state.to_string();
            snapshot.last_seen_ms = now;
            runtime.last_updated_ms = now;
        }
    }

    pub fn note_acs_inform(&self, feid: &str) {
        let now = now_ms();
        if let Some(runtime) = self
            .runtime
            .lock()
            .expect("femto cell state lock")
            .get_mut(feid)
        {
            runtime.last_acs_inform_ms = Some(now);
            runtime.last_updated_ms = now;
        }
    }

    pub fn observe_software_version(&self, feid: &str, software_version: String) {
        if let Some(runtime) = self
            .runtime
            .lock()
            .expect("femto cell state lock")
            .get_mut(feid)
        {
            runtime.software_version = Some(software_version);
            runtime.last_updated_ms = now_ms();
        }
    }

    pub fn observe_connection_request_url(&self, feid: &str, url: String) {
        if let Some(runtime) = self
            .runtime
            .lock()
            .expect("femto cell state lock")
            .get_mut(feid)
        {
            if runtime.connection_request_url.as_deref() != Some(url.as_str()) {
                log::info!("ACS: FAP {feid} advertised connection request URL {url}");
            }
            runtime.connection_request_url = Some(url);
            runtime.last_updated_ms = now_ms();
        }
    }

    pub fn note_acs_provisioned(&self, feid: &str) {
        if let Some(runtime) = self
            .runtime
            .lock()
            .expect("femto cell state lock")
            .get_mut(feid)
        {
            runtime.acs_provisioned = true;
            runtime.last_updated_ms = now_ms();
        }
    }

    pub fn complete_status_readback(&self, feid: &str, succeeded: bool) {
        if let Some(runtime) = self
            .runtime
            .lock()
            .expect("femto cell state lock")
            .get_mut(feid)
        {
            runtime.status_readback_generation =
                runtime.status_readback_generation.saturating_add(1);
            runtime.status_readback_succeeded = succeeded;
            runtime.last_updated_ms = now_ms();
        }
    }

    pub fn status_readback_state(&self, feid: &str) -> Option<(u64, bool)> {
        self.runtime
            .lock()
            .expect("femto cell state lock")
            .get(feid)
            .map(|runtime| {
                (
                    runtime.status_readback_generation,
                    runtime.status_readback_succeeded,
                )
            })
    }

    pub fn update_acs_status<'a>(
        &self,
        feid: &str,
        parameters: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) {
        let mut runtimes = self.runtime.lock().expect("femto cell state lock");
        let Some(runtime) = runtimes.get_mut(feid) else {
            return;
        };
        for (name, value) in parameters {
            if name.ends_with("GPSGotFix") {
                runtime.gps_status = Some(match value.to_ascii_lowercase().as_str() {
                    "1" | "true" => "Locked".to_string(),
                    "0" | "false" => "Not locked".to_string(),
                    _ => value.to_string(),
                });
            } else if name.ends_with("TimingGood") {
                runtime.timing_status = Some(match value.to_ascii_lowercase().as_str() {
                    "1" | "true" => "Synchronized".to_string(),
                    "0" | "false" => "Not synchronized".to_string(),
                    _ => value.to_string(),
                });
            } else if name.ends_with("GPSLatitude") {
                runtime.gps_latitude = gps_degrees_minutes_to_decimal(value, 90.0);
            } else if name.ends_with("GPSLongitude") {
                runtime.gps_longitude = gps_degrees_minutes_to_decimal(value, 180.0);
            } else if name.ends_with("LocationCheckStatus") {
                runtime.location_validation_status = Some(value.to_string());
            } else if name.ends_with("OneXSectorOperState") {
                runtime.radio_operational_state = Some(value.to_string());
            }
        }
        runtime.last_updated_ms = now_ms();
    }

    pub fn update_normalized_status(&self, feid: &str, status: NormalizedFapStatus) {
        if let Some(runtime) = self
            .runtime
            .lock()
            .expect("femto cell state lock")
            .get_mut(feid)
        {
            let has_gps_status = status.gps_status.is_some();
            if status.gps_status.is_some() {
                runtime.gps_status = status.gps_status;
            }
            if status.timing_status.is_some() {
                runtime.timing_status = status.timing_status;
            }
            if has_gps_status || status.gps_latitude.is_some() {
                runtime.gps_latitude = status.gps_latitude;
            }
            if has_gps_status || status.gps_longitude.is_some() {
                runtime.gps_longitude = status.gps_longitude;
            }
            if status.radio_operational_state.is_some() {
                runtime.radio_operational_state = status.radio_operational_state;
            }
            runtime.last_updated_ms = now_ms();
        }
    }

    pub fn cells(&self) -> Vec<FemtoCellSnapshot> {
        let runtime = self.runtime.lock().expect("femto cell state lock");
        let configured = self.configured.read().expect("femto cell state lock");
        let mut cells = configured
            .values()
            .map(|configured| {
                let live = &runtime[&configured.feid];
                FemtoCellSnapshot {
                    feid: configured.feid.clone(),
                    profile: configured.profile.clone(),
                    cell_id: configured.cell_id,
                    sector: configured.sector,
                    sid: configured.sid,
                    nid: configured.nid,
                    band_class: configured.band_class,
                    channel: configured.channel,
                    acs_configured: configured.acs_configured,
                    acs_provisioned: live.acs_provisioned,
                    sip_public_identity: live.sip_public_identity.clone(),
                    sip_peer: live.sip_peer,
                    inner_peer_address: live.inner_peer_address,
                    connection_request_url: live.connection_request_url.clone(),
                    connection_request_credentials: configured
                        .connection_request_credentials
                        .clone(),
                    gps_status: live.gps_status.clone(),
                    timing_status: live.timing_status.clone(),
                    gps_latitude: live.gps_latitude,
                    gps_longitude: live.gps_longitude,
                    software_version: live.software_version.clone(),
                    location_validation_status: live.location_validation_status.clone(),
                    radio_operational_state: live.radio_operational_state.clone(),
                    last_acs_inform_ms: live.last_acs_inform_ms,
                    last_updated_ms: live.last_updated_ms,
                }
            })
            .collect::<Vec<_>>();
        cells.sort_by_key(|cell| (cell.cell_id, cell.sector));
        cells
    }

    pub fn mobiles(&self) -> Vec<FemtoMobileSnapshot> {
        self.runtime
            .lock()
            .expect("femto cell state lock")
            .values()
            .flat_map(|runtime| runtime.mobiles.values().cloned())
            .collect()
    }

    pub fn request_radio_state(&self, feid: &str, enabled: bool) {
        if let Some(runtime) = self
            .runtime
            .lock()
            .expect("femto cell state lock")
            .get_mut(feid)
        {
            runtime.pending_radio_enabled = Some(enabled);
        }
    }

    pub fn pending_radio_state(&self, feid: &str) -> Option<bool> {
        self.runtime
            .lock()
            .expect("femto cell state lock")
            .get(feid)?
            .pending_radio_enabled
    }

    pub fn complete_radio_state(&self, feid: &str) {
        if let Some(runtime) = self
            .runtime
            .lock()
            .expect("femto cell state lock")
            .get_mut(feid)
        {
            runtime.pending_radio_enabled = None;
        }
    }

    pub fn request_reboot(&self, feid: &str) {
        if let Some(runtime) = self
            .runtime
            .lock()
            .expect("femto cell state lock")
            .get_mut(feid)
        {
            runtime.reboot_requested = true;
        }
    }

    pub fn reboot_requested(&self, feid: &str) -> bool {
        self.runtime
            .lock()
            .expect("femto cell state lock")
            .get(feid)
            .is_some_and(|runtime| runtime.reboot_requested)
    }

    pub fn complete_reboot(&self, feid: &str) {
        if let Some(runtime) = self
            .runtime
            .lock()
            .expect("femto cell state lock")
            .get_mut(feid)
        {
            runtime.reboot_requested = false;
        }
    }
}

fn esn_value(identity: &MobileIdentity) -> Option<u32> {
    match identity {
        MobileIdentity::Esn(value) => Some(*value),
        _ => None,
    }
}
fn meid_value(identity: &MobileIdentity) -> Option<String> {
    match identity {
        MobileIdentity::Meid(value) => Some(canonical_meid(value)),
        _ => None,
    }
}

/// The Airvana FAP delivers the MEID nibble-reversed in the A1 mobile identity,
/// so the decoded digits are in reverse order. Reverse the nibbles back to the
/// printed MEID order (e.g. `a0…`) that the HLR and A12 NAI use.
pub(crate) fn canonical_meid(digits: &[u8]) -> String {
    let transmitted: String = digits.iter().map(|byte| format!("{byte:02x}")).collect();
    transmitted.chars().rev().collect()
}

pub(crate) fn canonical_meid_identity(identity: MobileIdentity) -> MobileIdentity {
    match identity {
        MobileIdentity::Meid(digits) => {
            let hex = canonical_meid(&digits);
            let mut canonical = [0u8; 7];
            for (index, byte) in canonical.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).unwrap_or_default();
            }
            MobileIdentity::Meid(canonical)
        }
        other => other,
    }
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn gps_degrees_minutes_to_decimal(value: &str, maximum_degrees: f64) -> Option<f64> {
    let (sign, unsigned) = match value.as_bytes().first() {
        Some(b'-') => (-1.0, &value[1..]),
        Some(b'+') => (1.0, &value[1..]),
        _ => (1.0, value),
    };
    let whole_digits = unsigned.split_once('.')?.0.len();
    if whole_digits < 3 {
        return None;
    }
    let degree_digits = whole_digits - 2;
    let degrees = unsigned.get(..degree_digits)?.parse::<f64>().ok()?;
    let minutes = unsigned.get(degree_digits..)?.parse::<f64>().ok()?;
    if degrees > maximum_degrees || minutes >= 60.0 {
        return None;
    }
    Some(sign * (degrees + minutes / 60.0))
}

#[cfg(test)]
mod tests {
    use super::FemtoCellState;
    use super::canonical_meid;
    use super::canonical_meid_identity;
    use super::gps_degrees_minutes_to_decimal;
    use cdma_ios::MobileIdentity;

    #[test]
    fn imsi_only_observation_keeps_known_esn_and_meid() {
        let state = FemtoCellState::untracked();
        let imsi = "310005551239999";
        let meid = MobileIdentity::Meid([0xba, 0x87, 0xfe, 0x65, 0x00, 0x00, 0x0a]);
        state.observe_mobile(
            "test",
            imsi,
            Some(&MobileIdentity::Esn(0x80ab_cdef)),
            Some(&meid),
            "Registered",
        );
        state.observe_mobile("test", imsi, None, None, "Registered");
        let mobiles = state.mobiles();
        let mobile = mobiles
            .iter()
            .find(|m| m.imsi == imsi)
            .expect("mobile present");
        assert_eq!(mobile.esn, Some(0x80ab_cdef));
        assert_eq!(mobile.meid.as_deref(), Some("a0000056ef78ab"));
        state.observe_mobile(
            "test",
            imsi,
            Some(&MobileIdentity::Esn(0x1234_5678)),
            None,
            "Registered",
        );
        let mobiles = state.mobiles();
        let mobile = mobiles
            .iter()
            .find(|m| m.imsi == imsi)
            .expect("mobile present");
        assert_eq!(mobile.esn, Some(0x1234_5678));
        assert_eq!(mobile.meid.as_deref(), Some("a0000056ef78ab"));
    }

    #[test]
    fn canonical_meid_reverses_fap_nibble_order() {
        assert_eq!(
            canonical_meid(&[0xdc, 0x43, 0xba, 0x21, 0x00, 0x00, 0x0a]),
            "a0000012ab34cd"
        );
    }

    #[test]
    fn canonical_meid_identity_reverses_digits() {
        assert_eq!(
            canonical_meid_identity(MobileIdentity::Meid([
                0xdc, 0x43, 0xba, 0x21, 0x00, 0x00, 0x0a
            ])),
            MobileIdentity::Meid([0xa0, 0x00, 0x00, 0x12, 0xab, 0x34, 0xcd])
        );
    }

    #[test]
    fn converts_airave_gps_degrees_minutes() {
        let latitude = gps_degrees_minutes_to_decimal("+4000.300000", 90.0).unwrap();
        let longitude = gps_degrees_minutes_to_decimal("-07500.600000", 180.0).unwrap();

        assert!((latitude - 40.005).abs() < 0.000_000_1);
        assert!((longitude - -75.01).abs() < 0.000_000_1);
        assert!(gps_degrees_minutes_to_decimal("+4060.000000", 90.0).is_none());
    }
}
