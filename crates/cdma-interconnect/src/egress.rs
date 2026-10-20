use std::fmt::{Display, Formatter};

use async_trait::async_trait;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterconnectSms {
    pub sms_id: Uuid,
    pub source: String,
    pub destination: String,
    pub text: String,
    /// C.S0015 bearer data must be delivered verbatim.
    pub bearer_data: Option<Vec<u8>>,
    pub teleservice: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterconnectError {
    NotConnected,
    /// Permanent rejection. The message must not be retried.
    Rejected(String),
    /// Temporary rejection. The message may be retried.
    Transient(String),
}

impl Display for InterconnectError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConnected => f.write_str("interconnect not connected"),
            Self::Rejected(reason) => write!(f, "interconnect rejected: {reason}"),
            Self::Transient(reason) => write!(f, "interconnect unavailable: {reason}"),
        }
    }
}

#[async_trait]
pub trait InterconnectEgress: Send + Sync {
    fn is_available(&self) -> bool;

    /// `Ok` transfers delivery ownership to the hub. Do not retry.
    async fn submit(&self, message: InterconnectSms) -> Result<(), InterconnectError>;
}
