use cdma_smsc::client::{DeliveryChannel, RECONNECT_DELAY};
use cdma_smsc::proto::smsc_service_client::SmscServiceClient;
use cdma_smsc::proto::{DeliveryCommand, DeliveryOutcome};
use log::{debug, info, warn};
use tokio::sync::mpsc;

const COMMAND_QUEUE: usize = 32;

const OUTCOME_QUEUE: usize = 64;

pub struct DeliveryLink {
    pub commands: mpsc::Receiver<DeliveryCommand>,
    pub outcomes: mpsc::Sender<DeliveryOutcome>,
}

pub fn spawn(endpoint: String) -> DeliveryLink {
    let (command_tx, commands) = mpsc::channel(COMMAND_QUEUE);
    let (outcomes, mut outcome_rx) = mpsc::channel(OUTCOME_QUEUE);
    tokio::spawn(async move {
        loop {
            match connect(&endpoint).await {
                Some(mut channel) => {
                    info!("MSC: delivery channel to the SMSC is open");
                    let reporter = channel.reporter();
                    loop {
                        tokio::select! {
                            command = channel.next() => {
                                let Some(command) = command else { break };
                                if command_tx.send(command).await.is_err() {
                                    return;
                                }
                            }
                            outcome = outcome_rx.recv() => {
                                let Some(outcome) = outcome else { return };
                                if reporter.send(outcome).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    info!("MSC: the delivery channel closed");
                }
                None => debug!("MSC: SMSC not reachable for the delivery channel"),
            }
            tokio::time::sleep(RECONNECT_DELAY).await;
        }
    });
    DeliveryLink { commands, outcomes }
}

async fn connect(endpoint: &str) -> Option<DeliveryChannel> {
    let mut client = SmscServiceClient::connect(endpoint.to_string())
        .await
        .ok()?;
    match DeliveryChannel::open(&mut client).await {
        Ok(channel) => Some(channel),
        Err(status) => {
            warn!("MSC: could not open the delivery channel: {status}");
            None
        }
    }
}
