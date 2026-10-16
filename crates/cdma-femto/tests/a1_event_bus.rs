use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use cdma_events::proto::event_service_client::EventServiceClient;
use cdma_events::proto::{
    EventSource, ListenEventsRequest, MobileIdentity, PublishRequest, Subscriber, network_event,
};
use cdma_events::{EventBusConfig, EventBusServer, EventServiceServer, HlrEnricher};
use cdma_femto::event::{A1Context, a1_event};
use cdma_ios::femto_interface_msg::FemtoInterfaceMsg;
use tokio_stream::StreamExt;
use tonic::transport::Server;

/// The X.S0059-200 Table 2 reference body: Location Updating Request for
/// IMSI_S 9135198800, eleven information elements.
const REFERENCE_FEMTO: &[u8] = b"08 (=\"0 Location Updating Request\")\r\n\
0D=DQaWMRWJCPA=\r\n\
12=EgzJAOYAAAAAAQMBAAY=\r\n\
1F=HwE=\r\n\
0D=DQUFKa4zmQ==\r\n\
35=NQE=\r\n\
42=QgQBATjd\r\n\
28=KP0=\r\n\
40=QAA=\r\n\
41=QQUBAAAA/Q==\r\n\
0D=DQjxAAAAAAAA8A==\r\n";

async fn spawn_bus(enricher: Option<Arc<dyn HlrEnricher>>) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut bus = EventBusServer::new(EventBusConfig {
        subscriber_queue_capacity: 1024,
    });
    if let Some(enricher) = enricher {
        bus = bus.with_enricher(enricher);
    }
    let stream = tokio_stream::wrappers::TcpListenerStream::new(listener);
    tokio::spawn(async move {
        Server::builder()
            .add_service(EventServiceServer::new(bus))
            .serve_with_incoming(stream)
            .await
            .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    addr
}

fn reference_body() -> FemtoInterfaceMsg {
    FemtoInterfaceMsg::parse(REFERENCE_FEMTO).expect("reference body parses")
}

#[tokio::test]
async fn a1_events_reach_a_subscriber_with_the_sdu_and_ies_intact() {
    let addr = spawn_bus(None).await;
    let endpoint = format!("http://{addr}");

    let mut sub = EventServiceClient::connect(endpoint.clone()).await.unwrap();
    let mut stream = sub
        .listen_events(ListenEventsRequest {
            source_filter: vec![EventSource::An as i32, EventSource::Femto as i32],
        })
        .await
        .unwrap()
        .into_inner();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let message = reference_body();
    let published = a1_event(
        "0005B9AABBCC",
        &message,
        &A1Context {
            direction: cdma_events::proto::A1Direction::Uplink,
            ..A1Context::default()
        },
    );

    let mut publisher = EventServiceClient::connect(endpoint).await.unwrap();
    publisher
        .publish(PublishRequest {
            event: Some(published.clone()),
        })
        .await
        .unwrap();

    let received = stream.next().await.unwrap().unwrap();
    assert_eq!(received.source, EventSource::Femto as i32);
    assert!(
        received.sequence > 0,
        "bus assigns a sequence before fan-out"
    );

    let a1 = match received.body {
        Some(network_event::Body::A1(a1)) => a1,
        other => panic!("expected a top-level A1 body, got {other:?}"),
    };
    assert_eq!(a1.message_type_name, "Location Updating Request");
    assert_eq!(a1.peer_id, "0005B9AABBCC");
    assert_eq!(a1.direction, cdma_events::proto::A1Direction::Uplink as i32);

    assert_eq!(a1.sdu, message.to_layer3_sdu());
    assert_eq!(&a1.sdu[..3], &[0x05, 0x00, 0x08]);
    assert_eq!(a1.sdu_hex.len(), a1.sdu.len() * 2);

    assert_eq!(a1.information_elements.len(), message.ies.len());
    for (got, want) in a1.information_elements.iter().zip(&message.ies) {
        assert_eq!(got.iei, u32::from(want.iei));
        assert_eq!(got.content, want.content);
    }
    assert_eq!(a1.information_elements[0].name, "Mobile Identity");
}

struct StubEnricher;

#[async_trait::async_trait]
impl HlrEnricher for StubEnricher {
    async fn enrich(&self, identity: &mut MobileIdentity, subscriber: &mut Subscriber) {
        if identity.imsi == "9135198800" {
            *subscriber = Subscriber {
                subscriber_id: "sub-uuid-1".into(),
                phone_number: "+15551230000".into(),
                display_name: "Test Subscriber".into(),
                status: cdma_events::proto::SubscriberStatus::Active as i32,
            };
        }
    }
}

#[tokio::test]
async fn a1_events_are_hlr_enriched_from_their_mobile_identity() {
    let addr = spawn_bus(Some(Arc::new(StubEnricher))).await;
    let endpoint = format!("http://{addr}");

    let mut sub = EventServiceClient::connect(endpoint.clone()).await.unwrap();
    let mut stream = sub
        .listen_events(ListenEventsRequest {
            source_filter: vec![EventSource::Femto as i32],
        })
        .await
        .unwrap()
        .into_inner();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut publisher = EventServiceClient::connect(endpoint).await.unwrap();
    publisher
        .publish(PublishRequest {
            event: Some(a1_event(
                "0005B9AABBCC",
                &reference_body(),
                &A1Context::default(),
            )),
        })
        .await
        .unwrap();

    let received = stream.next().await.unwrap().unwrap();
    assert_eq!(received.identity.as_ref().unwrap().imsi, "9135198800");
    assert_eq!(received.identity.as_ref().unwrap().esn, 0x29AE_3399);
    let subscriber = received.subscriber.expect("HLR-resolved subscriber");
    assert_eq!(subscriber.subscriber_id, "sub-uuid-1");
    assert_eq!(subscriber.phone_number, "+15551230000");
}
