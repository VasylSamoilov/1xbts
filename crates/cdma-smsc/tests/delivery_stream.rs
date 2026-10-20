use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cdma_smsc::client::DeliveryChannel;
use cdma_smsc::delivery::{DeliveryHub, PushResult};
use cdma_smsc::model::{
    MoSmsFingerprint, SmsDeliveryAttempt, SmsDestination, SmsState, SmsSubmission,
};
use cdma_smsc::proto::smsc_service_client::SmscServiceClient;
use cdma_smsc::proto::{DeliveryCommand, DeliveryOutcome, delivery_outcome::Result as Outcome};
use cdma_smsc::repository::{CreateSubmissionOptions, SmscRepository};
use cdma_smsc::service::run_grpc_server_with_hub;
use uuid::Uuid;

struct NoStore;

#[async_trait]
impl SmscRepository for NoStore {
    async fn create_submission(
        &self,
        _: &str,
        _: SmsDestination,
        _: &str,
        _: Option<Uuid>,
        _: Option<Uuid>,
        _: CreateSubmissionOptions<'_>,
    ) -> Result<SmsSubmission, String> {
        Err("not used by these tests".into())
    }

    async fn create_or_get_recent_mo_submission(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: Option<Uuid>,
        _: Option<Uuid>,
        _: &MoSmsFingerprint,
    ) -> Result<(SmsSubmission, bool), String> {
        Err("not used by these tests".into())
    }

    async fn update_submission_state(
        &self,
        _: Uuid,
        _: SmsState,
        _: Option<String>,
    ) -> Result<SmsSubmission, String> {
        Err("not used by these tests".into())
    }

    async fn get_submission(&self, _: Uuid) -> Result<Option<SmsSubmission>, String> {
        Ok(None)
    }

    async fn list_submissions(
        &self,
        _: u32,
        _: u32,
        _: Option<&str>,
        _: Option<u32>,
        _: Option<&str>,
        _: Option<&str>,
        _: Option<&str>,
    ) -> Result<(Vec<SmsSubmission>, u32), String> {
        Ok((Vec::new(), 0))
    }

    async fn create_delivery_attempt(
        &self,
        _: Uuid,
        _: Option<Uuid>,
    ) -> Result<SmsDeliveryAttempt, String> {
        Err("not used by these tests".into())
    }

    async fn update_delivery_attempt_state(
        &self,
        _: Uuid,
        _: cdma_smsc::model::DeliveryAttemptState,
        _: Option<String>,
    ) -> Result<SmsDeliveryAttempt, String> {
        Err("not used by these tests".into())
    }

    async fn get_delivery_attempts(&self, _: Uuid) -> Result<Vec<SmsDeliveryAttempt>, String> {
        Ok(Vec::new())
    }

    async fn update_destination_subscriber(&self, _: Uuid, _: Uuid) -> Result<(), String> {
        Ok(())
    }
}

async fn smsc() -> (Arc<DeliveryHub>, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let hub = DeliveryHub::new();
    let serving = hub.clone();
    tokio::spawn(async move {
        let _ = run_grpc_server_with_hub(addr, Arc::new(NoStore), serving).await;
    });
    let endpoint = format!("http://{addr}");
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    (hub, endpoint)
}

fn command(sms_id: &str) -> DeliveryCommand {
    DeliveryCommand {
        sms_id: sms_id.to_string(),
        attempt_id: uuid::Uuid::new_v4().to_string(),
        target: None,
        originating_number: "7025550100".into(),
        text: "hello".into(),
        teleservice_id: None,
        raw_user_data: None,
    }
}

async fn wait_connected(hub: &DeliveryHub) {
    for _ in 0..200 {
        if hub.is_connected().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("no MSC ever registered");
}

#[tokio::test]
async fn the_smsc_can_push_a_delivery_to_a_connected_msc() {
    let (hub, endpoint) = smsc().await;
    let mut client = SmscServiceClient::connect(endpoint).await.unwrap();
    let mut channel = DeliveryChannel::open(&mut client).await.unwrap();
    wait_connected(&hub).await;

    assert_eq!(hub.push(command("abc")).await, PushResult::Sent);
    let received = channel.next().await.expect("a delivery");
    assert_eq!(received.sms_id, "abc");
    assert_eq!(received.text, "hello");
}

#[tokio::test]
async fn the_msc_can_report_back_up_the_same_stream() {
    let (hub, endpoint) = smsc().await;
    let mut client = SmscServiceClient::connect(endpoint).await.unwrap();
    let channel = DeliveryChannel::open(&mut client).await.unwrap();
    wait_connected(&hub).await;

    channel
        .report(DeliveryOutcome {
            sms_id: "abc".into(),
            attempt_id: uuid::Uuid::new_v4().to_string(),
            result: Outcome::Delivered as i32,
            detail: None,
        })
        .await;
    assert!(hub.is_connected().await);
}

#[tokio::test]
async fn nothing_is_pushed_while_no_msc_is_connected() {
    let (hub, _endpoint) = smsc().await;
    assert!(!hub.is_connected().await);
    assert_eq!(hub.push(command("abc")).await, PushResult::NoMsc);
}

#[tokio::test]
async fn an_msc_that_goes_away_stops_being_pushed_to() {
    let (hub, endpoint) = smsc().await;
    let mut client = SmscServiceClient::connect(endpoint.clone()).await.unwrap();
    let channel = DeliveryChannel::open(&mut client).await.unwrap();
    wait_connected(&hub).await;
    drop(channel);

    for _ in 0..200 {
        if hub.push(command("abc")).await == PushResult::NoMsc {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the hub kept pushing at a stream that had gone");
}

#[tokio::test]
async fn an_msc_that_reconnects_is_pushed_to_again() {
    let (hub, endpoint) = smsc().await;
    let mut first_client = SmscServiceClient::connect(endpoint.clone()).await.unwrap();
    let first = DeliveryChannel::open(&mut first_client).await.unwrap();
    wait_connected(&hub).await;
    drop(first);

    let mut second_client = SmscServiceClient::connect(endpoint).await.unwrap();
    let mut second = DeliveryChannel::open(&mut second_client).await.unwrap();
    wait_connected(&hub).await;

    for _ in 0..200 {
        if hub.push(command("after")).await == PushResult::Sent {
            assert_eq!(second.next().await.expect("a delivery").sms_id, "after");
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the reconnected MSC never became reachable");
}
