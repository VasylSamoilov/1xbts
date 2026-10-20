use std::sync::Arc;
use std::time::Duration;

use crate::dispatcher::Dispatcher;
use crate::model::DeliveryAttemptState;
use crate::repository::DeliveryCompletion;
use crate::repository::SmscRepository;

const PAGE_SIZE: u32 = 100;

#[derive(Debug, Clone)]
pub struct SweepConfig {
    pub enabled: bool,
    pub retry_after: Duration,
    pub interval: Duration,
    /// Bounds the time a delivery can remain in paging after its MSC dies.
    pub stale_after: Duration,
}

impl Default for SweepConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retry_after: Duration::from_secs(10),
            interval: Duration::from_secs(10),
            stale_after: Duration::from_secs(120),
        }
    }
}

pub struct Scheduler {
    repo: Arc<dyn SmscRepository>,
    dispatcher: Arc<Dispatcher>,
    config: SweepConfig,
}

impl Scheduler {
    pub fn new(
        repo: Arc<dyn SmscRepository>,
        dispatcher: Arc<Dispatcher>,
        config: SweepConfig,
    ) -> Self {
        Self {
            repo,
            dispatcher,
            config,
        }
    }

    pub async fn run(self) {
        if !self.config.enabled {
            log::info!("SMSC: the retry sweep is disabled");
            return;
        }
        self.recover_stuck().await;
        let mut ticker = tokio::time::interval(self.config.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            self.recover_stuck().await;
            self.retry_sweep().await;
        }
    }

    pub async fn retry_sweep(&self) {
        let cutoff = chrono::Utc::now()
            - chrono::Duration::from_std(self.config.retry_after)
                .unwrap_or(chrono::Duration::seconds(10));
        let mut offset = 0;
        loop {
            let (submissions, total) = match self
                .repo
                .list_submissions(PAGE_SIZE, offset, None, None, None, Some("accepted"), None)
                .await
            {
                Ok(page) => page,
                Err(e) => {
                    log::warn!("SMSC sweep: could not list submissions: {e}");
                    return;
                }
            };
            for submission in &submissions {
                let attempts = match self.repo.get_delivery_attempts(submission.sms_id).await {
                    Ok(attempts) => attempts,
                    Err(e) => {
                        log::warn!(
                            "SMSC sweep: could not read attempts for {}: {e}",
                            submission.sms_id
                        );
                        continue;
                    }
                };
                match attempts.iter().max_by_key(|a| a.attempt_number) {
                    None => {}
                    Some(latest) => {
                        if latest.state != DeliveryAttemptState::Failed {
                            continue;
                        }
                        if latest.completed_at.unwrap_or(latest.updated_at) > cutoff {
                            continue;
                        }
                    }
                }
                if let Err(reason) = self.dispatcher.dispatch(submission).await {
                    log::debug!(
                        "SMSC sweep: still holding {}: {reason:?}",
                        submission.sms_id
                    );
                }
            }
            offset += submissions.len() as u32;
            if offset >= total || submissions.is_empty() {
                break;
            }
        }
    }

    pub async fn recover_stuck(&self) {
        let cutoff = chrono::Utc::now()
            - chrono::Duration::from_std(self.config.stale_after)
                .unwrap_or(chrono::Duration::seconds(120));
        let mut offset = 0;
        let mut recovered = 0;
        loop {
            let (submissions, total) = match self
                .repo
                .list_submissions(PAGE_SIZE, offset, None, None, None, Some("paging"), None)
                .await
            {
                Ok(page) => page,
                Err(e) => {
                    log::warn!("SMSC recovery: could not list submissions: {e}");
                    return;
                }
            };
            for submission in &submissions {
                let Ok(attempts) = self.repo.get_delivery_attempts(submission.sms_id).await else {
                    continue;
                };
                let Some(latest) = attempts.iter().max_by_key(|a| a.attempt_number) else {
                    continue;
                };
                if latest.state != DeliveryAttemptState::Paging || latest.updated_at >= cutoff {
                    continue;
                }
                match self
                    .repo
                    .finish_delivery(
                        submission.sms_id,
                        latest.sms_delivery_attempt_id,
                        DeliveryCompletion::Retry(Some("no answer from the serving node".into())),
                    )
                    .await
                {
                    Ok(true) => recovered += 1,
                    Ok(false) => {}
                    Err(error) => log::warn!(
                        "SMSC recovery: could not recover {}: {error}",
                        submission.sms_id
                    ),
                }
            }
            offset += submissions.len() as u32;
            if offset >= total || submissions.is_empty() {
                break;
            }
        }
        if recovered > 0 {
            log::info!("SMSC: recovered {recovered} message(s) stuck in paging");
        }
    }
}
