use std::time::Duration;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::proto::{DeliveryCommand, DeliveryOutcome, smsc_service_client::SmscServiceClient};

pub const RECONNECT_DELAY: Duration = Duration::from_secs(5);

const OUTCOME_QUEUE: usize = 64;

const SUBMISSION_TIMEOUT: Duration = Duration::from_secs(5);

pub struct DeliveryChannel {
    commands: tonic::Streaming<DeliveryCommand>,
    outcomes: mpsc::Sender<DeliveryOutcome>,
}

impl DeliveryChannel {
    pub async fn open(
        client: &mut SmscServiceClient<tonic::transport::Channel>,
    ) -> Result<Self, tonic::Status> {
        let (outcomes, rx) = mpsc::channel(OUTCOME_QUEUE);
        let commands = client
            .delivery_stream(ReceiverStream::new(rx))
            .await?
            .into_inner();
        Ok(Self { commands, outcomes })
    }

    /// `None` requires reopening the closed stream.
    pub async fn next(&mut self) -> Option<DeliveryCommand> {
        match self.commands.message().await {
            Ok(next) => next,
            Err(status) => {
                log::warn!("MSC: the delivery stream failed: {status}");
                None
            }
        }
    }

    pub async fn report(&self, outcome: DeliveryOutcome) {
        if self.outcomes.send(outcome).await.is_err() {
            log::warn!("MSC: could not report a delivery outcome, the stream is gone");
        }
    }

    pub fn reporter(&self) -> mpsc::Sender<DeliveryOutcome> {
        self.outcomes.clone()
    }
}

#[derive(Debug, Clone)]
pub struct MoSubmission {
    pub originating_number: String,
    pub destination_number: String,
    pub text: String,
    pub originating_subscriber_id: Option<uuid::Uuid>,
    pub teleservice_id: Option<u16>,
    pub raw_user_data: Option<Vec<u8>>,
    pub fingerprint: crate::model::MoSmsFingerprint,
}

#[derive(Debug, Clone)]
pub struct MtSubmission {
    pub originating_number: String,
    pub destination: crate::model::SmsDestination,
    pub text: String,
    pub teleservice_id: Option<u16>,
    pub raw_user_data: Option<Vec<u8>>,
    pub destination_subscriber_id: Option<uuid::Uuid>,
    pub serving_node: Option<String>,
}

/// Submissions must be dispatched, not merely stored.
#[async_trait::async_trait]
pub trait MoSubmitter: Send + Sync {
    /// The boolean is false for a duplicate submission.
    async fn submit_mo(&self, message: MoSubmission) -> Result<(uuid::Uuid, bool), String>;

    async fn submit_mt(&self, message: MtSubmission) -> Result<uuid::Uuid, String>;
}

pub struct GrpcMoSubmitter {
    client: SmscServiceClient<tonic::transport::Channel>,
}

impl GrpcMoSubmitter {
    pub fn connect_lazy(endpoint: &str) -> Result<Self, tonic::transport::Error> {
        let channel = tonic::transport::Endpoint::from_shared(endpoint.to_string())?
            .timeout(SUBMISSION_TIMEOUT)
            .connect_lazy();
        Ok(Self {
            client: SmscServiceClient::new(channel),
        })
    }
}

#[async_trait::async_trait]
impl MoSubmitter for GrpcMoSubmitter {
    async fn submit_mo(&self, message: MoSubmission) -> Result<(uuid::Uuid, bool), String> {
        let request = crate::proto::SubmitMoRequest {
            originating_number: message.originating_number,
            destination_number: message.destination_number,
            text: message.text,
            originating_subscriber_id: message.originating_subscriber_id.map(|id| id.to_string()),
            teleservice_id: message.teleservice_id.map(u32::from),
            raw_user_data: message.raw_user_data,
            fingerprint: Some(crate::proto::MoSmsFingerprint {
                teleservice_id: u32::from(message.fingerprint.teleservice_id),
                message_type: u32::from(message.fingerprint.message_type),
                message_id: u32::from(message.fingerprint.message_id),
            }),
        };
        let response = self
            .client
            .clone()
            .submit_mo(request)
            .await
            .map_err(|e| format!("SubmitMo failed: {e}"))?
            .into_inner();
        let sms_id = uuid::Uuid::parse_str(&response.sms_id)
            .map_err(|e| format!("the SMSC returned an unreadable sms_id: {e}"))?;
        Ok((sms_id, response.accepted))
    }

    async fn submit_mt(&self, message: MtSubmission) -> Result<uuid::Uuid, String> {
        use crate::model::SmsDestination;
        use crate::proto::submit_mt_request::Destination;
        let request = crate::proto::SubmitMtRequest {
            originating_number: message.originating_number,
            destination: Some(match message.destination {
                SmsDestination::PhoneNumber(number) => Destination::Number(number),
                SmsDestination::Esn(esn) => Destination::Esn(u64::from(esn)),
                SmsDestination::Imsi(imsi) => Destination::Imsi(imsi),
            }),
            text: message.text,
            teleservice_id: message.teleservice_id.map(u32::from),
            raw_user_data: message.raw_user_data,
            destination_subscriber_id: message.destination_subscriber_id.map(|id| id.to_string()),
            serving_node: message.serving_node,
        };
        let response = self
            .client
            .clone()
            .submit_mt(request)
            .await
            .map_err(|e| format!("SubmitMt failed: {e}"))?
            .into_inner();
        uuid::Uuid::parse_str(&response.sms_id)
            .map_err(|e| format!("the SMSC returned an unreadable sms_id: {e}"))
    }
}
