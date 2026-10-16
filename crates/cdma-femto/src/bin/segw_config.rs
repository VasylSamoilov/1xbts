use std::time::Duration;

use cdma_femto::{
    config::{ConfigError, SegwConfig},
    segw::{SegwController, SegwError},
};

const CONFIG_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, thiserror::Error)]
enum ControllerError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Segw(#[from] SegwError),
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let config_path =
        std::env::var("FEMTO_CONFIG").unwrap_or_else(|_| "config/femto.json".to_string());
    let mut applied_config = None;
    let mut logged_error = None;

    loop {
        match apply_if_changed(&config_path, applied_config.as_deref()) {
            Ok(Some(rendered)) => {
                log::info!("SeGW configuration updated from {config_path}");
                applied_config = Some(rendered);
                logged_error = None;
            }
            Ok(None) => {
                logged_error = None;
            }
            Err(error) => {
                let message = error.to_string();
                if logged_error.as_deref() != Some(message.as_str()) {
                    log::error!("SeGW configuration unavailable: {message}");
                    logged_error = Some(message);
                }
            }
        }
        tokio::time::sleep(CONFIG_POLL_INTERVAL).await;
    }
}

fn apply_if_changed(
    config_path: &str,
    applied_config: Option<&str>,
) -> Result<Option<String>, ControllerError> {
    let config = SegwConfig::load(config_path)?;
    let controller = SegwController::new(&config);
    let rendered = controller.render_swanctl_conf()?;
    if applied_config == Some(rendered.as_str()) {
        return Ok(None);
    }
    controller.apply()?;
    Ok(Some(rendered))
}

#[cfg(test)]
mod tests {
    use std::{fs, time::SystemTime};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn applies_partial_segw_config_only_when_changed() {
        let suffix = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "cdma-femto-segw-config-{}-{suffix}",
            std::process::id()
        ));
        let output_directory = directory.join("swanctl");
        fs::create_dir_all(&directory).expect("create test directory");
        let config_path = directory.join("femto.json");
        let config = serde_json::json!({
            "segw": {
                "swanctl_dir": output_directory,
                "vip_subnet": "198.51.100.0/24",
                "vip_dns": "192.0.2.53",
                "connections": [{
                    "feid": "0005B9AABBCC",
                    "profile": "legacy",
                    "ike_group_key": "test-group-key",
                    "eap_identity": "fap@example.test",
                    "eap_password": "test-eap-password"
                }]
            }
        });
        fs::write(
            &config_path,
            serde_json::to_vec(&config).expect("serialize test config"),
        )
        .expect("write test config");

        let rendered = apply_if_changed(config_path.to_str().expect("UTF-8 test path"), None)
            .expect("apply initial config")
            .expect("initial config changed");
        let output_path = output_directory.join("swanctl.conf");
        assert!(output_path.is_file());
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&output_path)
                .expect("read output metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(
            apply_if_changed(
                config_path.to_str().expect("UTF-8 test path"),
                Some(&rendered),
            )
            .expect("check unchanged config")
            .is_none()
        );

        fs::remove_dir_all(directory).expect("remove test directory");
    }
}
