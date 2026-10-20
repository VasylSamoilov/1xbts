use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};

use crate::proto::{DeliveryCommand, DeliveryOutcome};

const PUSH_QUEUE: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushResult {
    Sent,
    NoMsc,
    Backlogged,
}

#[derive(Default)]
pub struct DeliveryHub {
    msc: Mutex<Option<mpsc::Sender<DeliveryCommand>>>,
}

impl DeliveryHub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Replace the previous stream so a restart does not wait for its TCP timeout.
    pub async fn attach(&self) -> mpsc::Receiver<DeliveryCommand> {
        let (tx, rx) = mpsc::channel(PUSH_QUEUE);
        *self.msc.lock().await = Some(tx);
        rx
    }

    /// Remove only the matching stream so a stale disconnect cannot remove its replacement.
    pub async fn detach(&self) {
        let mut slot = self.msc.lock().await;
        if slot.as_ref().is_some_and(|tx| tx.is_closed()) {
            *slot = None;
        }
    }

    pub async fn is_connected(&self) -> bool {
        self.msc
            .lock()
            .await
            .as_ref()
            .is_some_and(|tx| !tx.is_closed())
    }

    pub async fn push(&self, command: DeliveryCommand) -> PushResult {
        let slot = self.msc.lock().await;
        let Some(tx) = slot.as_ref() else {
            return PushResult::NoMsc;
        };
        match tx.try_send(command) {
            Ok(()) => PushResult::Sent,
            Err(mpsc::error::TrySendError::Full(_)) => PushResult::Backlogged,
            Err(mpsc::error::TrySendError::Closed(_)) => PushResult::NoMsc,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptResult {
    Delivered,
    NotReachable(Option<String>),
    Failed(Option<String>),
}

impl AttemptResult {
    pub fn from_outcome(outcome: &DeliveryOutcome) -> Self {
        use crate::proto::delivery_outcome::Result as Wire;
        match Wire::try_from(outcome.result) {
            Ok(Wire::Delivered) => Self::Delivered,
            Ok(Wire::Failed) => Self::Failed(outcome.detail.clone()),
            // Unknown outcomes remain retryable to avoid losing messages across version changes.
            _ => Self::NotReachable(outcome.detail.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(id: &str) -> DeliveryCommand {
        DeliveryCommand {
            sms_id: id.to_string(),
            attempt_id: uuid::Uuid::new_v4().to_string(),
            target: None,
            originating_number: "7025550100".into(),
            text: "hello".into(),
            teleservice_id: None,
            raw_user_data: None,
        }
    }

    #[tokio::test]
    async fn nothing_can_be_pushed_before_an_msc_arrives() {
        let hub = DeliveryHub::new();
        assert!(!hub.is_connected().await);
        assert_eq!(hub.push(command("a")).await, PushResult::NoMsc);
    }

    #[tokio::test]
    async fn a_delivery_reaches_the_connected_msc() {
        let hub = DeliveryHub::new();
        let mut rx = hub.attach().await;
        assert!(hub.is_connected().await);
        assert_eq!(hub.push(command("a")).await, PushResult::Sent);
        assert_eq!(rx.recv().await.unwrap().sms_id, "a");
    }

    #[tokio::test]
    async fn an_msc_that_dials_back_in_replaces_its_own_dead_stream() {
        let hub = DeliveryHub::new();
        let first = hub.attach().await;
        let mut second = hub.attach().await;
        drop(first);
        assert_eq!(hub.push(command("a")).await, PushResult::Sent);
        assert_eq!(second.recv().await.unwrap().sms_id, "a");
    }

    #[tokio::test]
    async fn a_dropped_stream_leaves_nothing_connected() {
        let hub = DeliveryHub::new();
        let rx = hub.attach().await;
        drop(rx);
        assert_eq!(hub.push(command("a")).await, PushResult::NoMsc);
        hub.detach().await;
        assert!(!hub.is_connected().await);
    }

    #[tokio::test]
    async fn an_msc_that_stops_draining_is_reported_rather_than_buffered() {
        let hub = DeliveryHub::new();
        let _rx = hub.attach().await;
        for n in 0..PUSH_QUEUE {
            assert_eq!(hub.push(command(&n.to_string())).await, PushResult::Sent);
        }
        assert_eq!(
            hub.push(command("one-too-many")).await,
            PushResult::Backlogged
        );
    }

    #[test]
    fn an_unknown_result_is_treated_as_worth_retrying() {
        let outcome = DeliveryOutcome {
            sms_id: "a".into(),
            attempt_id: uuid::Uuid::new_v4().to_string(),
            result: 99,
            detail: None,
        };
        assert!(matches!(
            AttemptResult::from_outcome(&outcome),
            AttemptResult::NotReachable(_)
        ));
    }
}
