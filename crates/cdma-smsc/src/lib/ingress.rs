use std::sync::Arc;

use async_trait::async_trait;
use cdma_interconnect::{IncomingSms, MtSink};
use uuid::Uuid;

use crate::model::SmsDestination;
use crate::repository::{CreateSubmissionOptions, SmscRepository};

pub struct RepositoryMtSink {
    smsc: Arc<dyn SmscRepository>,
}

impl RepositoryMtSink {
    pub fn new(smsc: Arc<dyn SmscRepository>) -> Self {
        Self { smsc }
    }
}

#[async_trait]
impl MtSink for RepositoryMtSink {
    async fn deliver(&self, message: IncomingSms) -> Result<Uuid, String> {
        let submission = self
            .smsc
            .create_submission(
                &message.source,
                SmsDestination::PhoneNumber(message.destination.clone()),
                &message.text,
                None,
                None,
                CreateSubmissionOptions {
                    teleservice_id: message.teleservice,
                    raw_user_data: message.bearer_data.as_deref(),
                    // Inbound interconnect messages must not be routed back out, or they loop.
                    from_interconnect: true,
                },
            )
            .await?;
        Ok(submission.sms_id)
    }
}
