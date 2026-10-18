use crate::config::{SecurityProfile, SegwConfig, SegwConnection};
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

const DEFAULT_STANDARD_PROPOSAL: &str = "aes256-sha256-modp2048";
const DEFAULT_LEGACY_PROPOSAL: &str = "3des-sha1-modp1024";

#[derive(Debug, thiserror::Error)]
pub enum SegwError {
    #[error(
        "connection {feid}: legacy profile requires ike_group_key and eap_password in config/femto.local.json"
    )]
    MissingLegacyCredentials { feid: String },
    #[error(
        "connection {feid}: standard profile requires segw_cert, segw_key, and fap_ca_cert paths"
    )]
    MissingStandardMaterial { feid: String },
    #[error("writing swanctl config to {path}: {source}")]
    Write {
        path: String,
        source: std::io::Error,
    },
}

const VIP_POOL_NAME: &str = "femto-vip";

// DPD doubles as a NAT keepalive: the FAP's reply refreshes its UDP-4500 mapping,
// holding the tunnel and its VIP instead of forcing a fresh IKE_SA on a new pool address.
const DPD_KEEPALIVE_DELAY: &str = "30s";

pub struct SegwController {
    default_profile: SecurityProfile,
    swanctl_dir: PathBuf,
    connections: Vec<SegwConnection>,
    vip_subnet: Option<String>,
    vip_dns: Option<String>,
    vip_netmask: Option<String>,
}

impl SegwController {
    pub fn new(config: &SegwConfig) -> Self {
        Self {
            default_profile: config.profile_default,
            swanctl_dir: PathBuf::from(&config.swanctl_dir),
            connections: config.connections.clone(),
            vip_subnet: config.vip_subnet.clone(),
            vip_dns: config.vip_dns.clone(),
            vip_netmask: config.vip_netmask.clone(),
        }
    }

    fn pool_line(&self) -> String {
        if self.vip_subnet.is_some() {
            format!("    pools = {VIP_POOL_NAME}\n")
        } else {
            String::new()
        }
    }

    fn child_ts(&self) -> String {
        if self.vip_subnet.is_some() {
            "\x20       local_ts = 0.0.0.0/0\n\
             \x20       remote_ts = dynamic\n"
                .to_string()
        } else {
            String::new()
        }
    }

    fn render_pools(&self) -> String {
        let Some(subnet) = self.vip_subnet.as_deref() else {
            return String::new();
        };
        let netmask = self
            .vip_netmask
            .as_deref()
            .map(|mask| format!("      netmask = {mask}\n"))
            .unwrap_or_default();
        let dns = self
            .vip_dns
            .as_deref()
            .map(|d| format!("      dns = {d}\n"))
            .unwrap_or_default();
        format!(
            "pools {{\n  {VIP_POOL_NAME} {{\n      addrs = {subnet}\n{netmask}{dns}  }}\n}}\n\n"
        )
    }

    pub fn default_profile(&self) -> SecurityProfile {
        self.default_profile
    }

    pub fn render_swanctl_conf(&self) -> Result<String, SegwError> {
        let mut connections = String::new();
        let mut secrets = String::new();
        for conn in &self.connections {
            self.render_connection(conn, &mut connections, &mut secrets)?;
        }
        let pools = self.render_pools();
        Ok(format!(
            "{pools}connections {{\n{connections}}}\n\nsecrets {{\n{secrets}}}\n"
        ))
    }

    fn render_connection(
        &self,
        conn: &SegwConnection,
        connections: &mut String,
        secrets: &mut String,
    ) -> Result<(), SegwError> {
        let name = sanitize(&conn.feid);
        let pool_line = self.pool_line();
        let child_ts = self.child_ts();
        match conn.profile {
            SecurityProfile::Legacy => {
                let group_key = conn.ike_group_key.as_deref().ok_or_else(|| {
                    SegwError::MissingLegacyCredentials {
                        feid: conn.feid.clone(),
                    }
                })?;
                let eap_password = conn.eap_password.as_deref().ok_or_else(|| {
                    SegwError::MissingLegacyCredentials {
                        feid: conn.feid.clone(),
                    }
                })?;
                let eap_id = conn.eap_identity.as_deref().unwrap_or(&conn.feid);
                let ike_id_line = conn
                    .ike_identity
                    .as_deref()
                    .map(|id| format!("\x20     id = {id}\n"))
                    .unwrap_or_default();
                let proposal = conn.proposal.as_deref().unwrap_or(DEFAULT_LEGACY_PROPOSAL);
                let esp = esp_from_ike(proposal);
                connections.push_str(&format!(
                    "  fap-{name} {{\n\
                     \x20   version = 2\n\
                     \x20   proposals = {proposal}\n\
                     \x20   remote_addrs = %any\n\
                     \x20   dpd_delay = {DPD_KEEPALIVE_DELAY}\n\
                     {pool_line}\
                     \x20   local {{\n\
                     \x20     auth = psk\n\
                     \x20   }}\n\
                     \x20   remote {{\n\
                     \x20     auth = eap-md5\n\
                     {ike_id_line}\
                     \x20     eap_id = {eap_id}\n\
                     \x20   }}\n\
                     \x20   children {{\n\
                     \x20     fap-{name} {{\n\
                     \x20       esp_proposals = {esp}\n\
                     {child_ts}\
                     \x20     }}\n\
                     \x20   }}\n\
                     \x20 }}\n"
                ));
                // StrongSwan's config parser reads a value to end-of-line, so
                // each setting must be on its own line - an inline
                // `{ id = .. secret = .. }` makes `id` swallow the `secret`.
                secrets.push_str(&format!(
                    "  ike-fap-{name} {{\n\
                     \x20   secret = \"{group_key}\"\n\
                     \x20 }}\n\
                     \x20 eap-fap-{name} {{\n\
                     \x20   id = {eap_id}\n\
                     \x20   secret = \"{eap_password}\"\n\
                     \x20 }}\n"
                ));
            }
            SecurityProfile::Standard => {
                let (segw_cert, segw_key, fap_ca) =
                    match (&conn.segw_cert, &conn.segw_key, &conn.fap_ca_cert) {
                        (Some(c), Some(k), Some(ca)) => (c, k, ca),
                        _ => {
                            return Err(SegwError::MissingStandardMaterial {
                                feid: conn.feid.clone(),
                            });
                        }
                    };
                let proposal = conn
                    .proposal
                    .as_deref()
                    .unwrap_or(DEFAULT_STANDARD_PROPOSAL);
                let esp = esp_from_ike(proposal);
                connections.push_str(&format!(
                    "  fap-{name} {{\n\
                     \x20   version = 2\n\
                     \x20   proposals = {proposal}\n\
                     \x20   remote_addrs = %any\n\
                     \x20   dpd_delay = {DPD_KEEPALIVE_DELAY}\n\
                     {pool_line}\
                     \x20   local {{\n\
                     \x20     auth = pubkey\n\
                     \x20     certs = {segw_cert}\n\
                     \x20   }}\n\
                     \x20   remote {{\n\
                     \x20     auth = pubkey\n\
                     \x20     cacerts = {fap_ca}\n\
                     \x20   }}\n\
                     \x20   children {{\n\
                     \x20     fap-{name} {{\n\
                     \x20       esp_proposals = {esp}\n\
                     {child_ts}\
                     \x20     }}\n\
                     \x20   }}\n\
                     \x20 }}\n"
                ));
                secrets.push_str(&format!(
                    "  rsa-fap-{name} {{\n\
                     \x20   file = {segw_key}\n\
                     \x20 }}\n"
                ));
            }
        }
        Ok(())
    }

    pub fn apply(&self) -> Result<PathBuf, SegwError> {
        let conf = self.render_swanctl_conf()?;
        std::fs::create_dir_all(&self.swanctl_dir).map_err(|source| SegwError::Write {
            path: self.swanctl_dir.display().to_string(),
            source,
        })?;
        let path = self.swanctl_dir.join("swanctl.conf");
        write_atomic(&path, conf.as_bytes())?;
        log::info!(
            "SeGW: wrote {} FAP connection profile(s) to {}",
            self.connections.len(),
            path.display()
        );
        Ok(path)
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), SegwError> {
    let temporary_path = path.with_extension("conf.tmp");
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary_path)
        .map_err(|source| SegwError::Write {
            path: temporary_path.display().to_string(),
            source,
        })?;
    file.write_all(bytes).map_err(|source| SegwError::Write {
        path: temporary_path.display().to_string(),
        source,
    })?;
    file.sync_all().map_err(|source| SegwError::Write {
        path: temporary_path.display().to_string(),
        source,
    })?;
    std::fs::rename(&temporary_path, path).map_err(|source| SegwError::Write {
        path: path.display().to_string(),
        source,
    })
}

fn esp_from_ike(ike: &str) -> String {
    ike.split('-')
        .filter(|part| {
            !part.starts_with("modp") && !part.starts_with("ecp") && !part.starts_with("prf")
        })
        .collect::<Vec<_>>()
        .join("-")
}

fn sanitize(feid: &str) -> String {
    feid.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SegwConnection;

    fn legacy_conn() -> SegwConnection {
        SegwConnection {
            feid: "0005B9AABBCC".to_string(),
            profile: SecurityProfile::Legacy,
            proposal: Some("3des-sha1-modp1024".to_string()),
            fap_ca_cert: None,
            segw_cert: None,
            segw_key: None,
            ike_group_key: Some("test-group-key".to_string()),
            eap_identity: Some("fap@realm".to_string()),
            ike_identity: Some("fap-ike@realm".to_string()),
            eap_password: Some("test-eap-pw".to_string()),
        }
    }

    fn controller(conns: Vec<SegwConnection>) -> SegwController {
        SegwController {
            default_profile: SecurityProfile::Legacy,
            swanctl_dir: PathBuf::from("/tmp/does-not-matter"),
            connections: conns,
            vip_subnet: None,
            vip_dns: None,
            vip_netmask: None,
        }
    }

    #[test]
    fn renders_legacy_connection_with_psk_and_eap_secrets() {
        let conf = controller(vec![legacy_conn()])
            .render_swanctl_conf()
            .expect("render");
        assert!(conf.contains("proposals = 3des-sha1-modp1024"));
        assert!(conf.contains("dpd_delay = 30s"));
        assert!(conf.contains("esp_proposals = 3des-sha1"));
        assert!(conf.contains("auth = eap-md5"));
        assert!(conf.contains("eap_id = fap@realm"));
        assert!(conf.contains("id = fap-ike@realm"));
        assert!(conf.contains("secret = \"test-group-key\""));
        assert!(conf.contains("secret = \"test-eap-pw\""));
    }

    #[test]
    fn renders_virtual_ip_pool_with_dns() {
        let mut controller = controller(vec![legacy_conn()]);
        controller.vip_subnet = Some("198.51.100.0/24".to_string());
        controller.vip_dns = Some("192.0.2.53".to_string());

        let conf = controller.render_swanctl_conf().expect("render");

        assert!(conf.contains("addrs = 198.51.100.0/24"));
        assert!(conf.contains("dns = 192.0.2.53"));
        assert!(conf.contains("pools = femto-vip"));
    }

    #[test]
    fn virtual_ip_netmask_is_opt_in_for_plus_firmware() {
        let mut controller = controller(vec![legacy_conn()]);
        controller.vip_subnet = Some("198.51.100.0/24".to_string());
        let original = controller
            .render_swanctl_conf()
            .expect("render without mask");
        assert!(!original.contains("netmask ="));
        controller.vip_netmask = Some("255.255.255.0".to_string());
        let conf = controller.render_swanctl_conf().expect("render with mask");
        assert!(conf.contains("addrs = 198.51.100.0/24\n      netmask = 255.255.255.0\n"));
        controller.vip_subnet = None;
        assert!(
            !controller
                .render_swanctl_conf()
                .expect("no pool")
                .contains("netmask =")
        );
    }

    #[test]
    fn legacy_without_credentials_errors_rather_than_emitting_blank_secret() {
        let mut conn = legacy_conn();
        conn.ike_group_key = None;
        let err = controller(vec![conn]).render_swanctl_conf().unwrap_err();
        assert!(matches!(err, SegwError::MissingLegacyCredentials { .. }));
    }

    #[test]
    fn renders_standard_certificate_connection() {
        let conn = SegwConnection {
            feid: "STD1".to_string(),
            profile: SecurityProfile::Standard,
            proposal: None,
            fap_ca_cert: Some("fap-ca.pem".to_string()),
            segw_cert: Some("segw.pem".to_string()),
            segw_key: Some("segw-key.pem".to_string()),
            ike_group_key: None,
            eap_identity: None,
            ike_identity: None,
            eap_password: None,
        };
        let conf = controller(vec![conn])
            .render_swanctl_conf()
            .expect("render");
        assert!(conf.contains("auth = pubkey"));
        assert!(conf.contains("certs = segw.pem"));
        assert!(conf.contains("cacerts = fap-ca.pem"));
        assert!(conf.contains("file = segw-key.pem"));
        assert!(conf.contains("proposals = aes256-sha256-modp2048"));
    }
}
