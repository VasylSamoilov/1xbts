pub mod client;
pub mod config;
pub mod delivery;
pub mod dispatcher;
pub mod ingress;
pub mod model;
pub mod node;
pub mod repository;
pub mod routing;
pub mod scheduler;
pub mod service;

pub use config::SmscNodeConfig;
pub use node::{resolve_config_dir, run_node};

pub mod proto {
    tonic::include_proto!("smsc.v1");
}
