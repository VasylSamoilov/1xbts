use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BindRole {
    /// GSMA IR.75 §7.3 recommends that the site listen for the hub's bind.
    Server,
    Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterconnectConfig {
    pub role: BindRole,
    #[serde(default)]
    pub bind_addr: Option<SocketAddr>,
    #[serde(default)]
    pub hub_addr: Option<SocketAddr>,
    pub system_id: String,
    pub password: String,
}

impl InterconnectConfig {
    pub fn endpoint(&self) -> Option<SocketAddr> {
        match self.role {
            BindRole::Server => self.bind_addr,
            BindRole::Client => self.hub_addr,
        }
    }
}
