use async_trait::async_trait;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingSms {
    pub source: String,
    pub destination: String,
    pub text: String,
    /// C.S0015 bearer data must be delivered verbatim.
    pub bearer_data: Option<Vec<u8>>,
    pub teleservice: Option<u16>,
}

#[async_trait]
pub trait MtSink: Send + Sync {
    /// Return an error only if storage fails. An unreachable handset must not reject acceptance.
    async fn deliver(&self, message: IncomingSms) -> Result<Uuid, String>;
}
