use cdma_femto::{FemtoConfig, FemtoService};

#[tokio::main]
async fn main() {
    env_logger::init();

    let config_path =
        std::env::var("FEMTO_CONFIG").unwrap_or_else(|_| "config/femto.json".to_string());

    let config = match FemtoConfig::load(&config_path) {
        Ok(config) => config,
        Err(err) => {
            log::error!("{err}");
            std::process::exit(1);
        }
    };

    let service = FemtoService::new(config);
    if let Err(err) = service.run().await {
        log::error!("femtocell service failed to start: {err}");
        std::process::exit(1);
    }
}
