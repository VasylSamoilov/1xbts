use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cdma_interconnect::{BindRole, IncomingSms, InterconnectConfig, MtSink};
use cdma_interconnect::{InterconnectEgress, InterconnectError, InterconnectSms};
use cdma_interconnect_smpp::InterconnectLink;
use smpp_core::{
    Action, Address, CommandStatus, Config, MessageBody, PduReader, PduWriter, Session,
};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

#[derive(Default)]
struct Collector {
    taken: Mutex<Vec<IncomingSms>>,
    refuse: bool,
}

#[async_trait]
impl MtSink for Collector {
    async fn deliver(&self, message: IncomingSms) -> Result<Uuid, String> {
        if self.refuse {
            return Err("the database is down".into());
        }
        self.taken.lock().unwrap().push(message);
        Ok(Uuid::new_v4())
    }
}

struct FakeHub {
    session: Session,
    reader: PduReader<tokio::io::ReadHalf<TcpStream>>,
    writer: PduWriter<tokio::io::WriteHalf<TcpStream>>,
}

impl FakeHub {
    async fn accept(listener: TcpListener, password: &str) -> Self {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, write) = tokio::io::split(stream);
        let mut hub = Self {
            session: Session::new(Config::smsc("1xbts-hub"), Instant::now()),
            reader: PduReader::new(read),
            writer: PduWriter::new(write),
        };
        loop {
            let pdu = hub.reader.next().await.unwrap().expect("a bind");
            let mut done = false;
            for action in hub.session.handle(pdu, Instant::now()) {
                if let Action::BindRequest {
                    sequence,
                    system_id,
                    password: offered,
                } = action
                {
                    assert_eq!(offered, password, "the site offered the wrong password");
                    let reply = hub.session.accept_bind(sequence, system_id, Instant::now());
                    hub.writer.send(&reply).await.unwrap();
                    done = true;
                }
            }
            if done {
                return hub;
            }
        }
    }

    async fn take_submission(&mut self, status: CommandStatus) -> MessageBody {
        loop {
            let pdu = self.reader.next().await.unwrap().expect("a submission");
            for action in self.session.handle(pdu, Instant::now()) {
                match action {
                    Action::Message { sequence, body } => {
                        let reply = self.session.respond_to_message(
                            sequence,
                            status,
                            "hub-message-id".into(),
                        );
                        self.writer.send(&reply).await.unwrap();
                        return *body;
                    }
                    Action::Send(pdu) => self.writer.send(&pdu).await.unwrap(),
                    _ => {}
                }
            }
        }
    }

    async fn deliver(&mut self, from: &str, to: &str, text: &str) -> CommandStatus {
        let (data_coding, text) = smpp_core::text::encode(text).unwrap();
        let body = MessageBody {
            source: Address::unknown(from),
            destination: Address::unknown(to),
            data_coding,
            text,
            ..MessageBody::default()
        };
        let (_, pdu) = self.session.send_message(body, Instant::now()).unwrap();
        self.writer.send(&pdu).await.unwrap();
        loop {
            let pdu = self.reader.next().await.unwrap().expect("an answer");
            let status = pdu.status;
            let actions = self.session.handle(pdu, Instant::now());
            for action in actions {
                if let Action::MessageAck { status, .. } = action {
                    return status;
                }
                if let Action::Send(pdu) = action {
                    self.writer.send(&pdu).await.unwrap();
                }
            }
            let _ = status;
        }
    }
}

async fn link(sink: Arc<Collector>) -> (InterconnectLink, TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let password = "s3cret08".to_string();
    let link = InterconnectLink::start(
        InterconnectConfig {
            role: BindRole::Client,
            bind_addr: None,
            hub_addr: Some(address),
            system_id: "kw6xyz-lv".into(),
            password: password.clone(),
        },
        sink,
    );
    (link, listener, password)
}

fn sms(source: &str, destination: &str, text: &str) -> InterconnectSms {
    InterconnectSms {
        sms_id: Uuid::new_v4(),
        source: source.into(),
        destination: destination.into(),
        text: text.into(),
        bearer_data: None,
        teleservice: None,
    }
}

async fn wait_bound(link: &InterconnectLink) {
    for _ in 0..200 {
        if link.is_available() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the link never bound");
}

#[tokio::test]
async fn a_message_for_another_operator_goes_out_and_is_acknowledged() {
    let sink = Arc::new(Collector::default());
    let (link, listener, password) = link(sink.clone()).await;
    let mut hub = FakeHub::accept(listener, &password).await;
    wait_bound(&link).await;

    let mut message = sms("7025550100", "4155550118", "Hello 世界 €");
    let native = vec![0, 255, 128, 0];
    const TELESERVICE: u16 = 0x1002;
    message.bearer_data = Some(native.clone());
    message.teleservice = Some(TELESERVICE);
    let submitting = tokio::spawn(async move { link.submit(message).await });
    let body = hub.take_submission(CommandStatus::Ok).await;
    assert_eq!(body.source.digits, "7025550100");
    assert_eq!(body.destination.digits, "4155550118");
    assert_eq!(smpp_core::text::decode(&body).unwrap(), "Hello 世界 €");
    assert_eq!(body.bearer_data, Some(native));
    assert_eq!(body.teleservice, Some(TELESERVICE));
    assert_eq!(submitting.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn a_refused_message_is_never_offered_again() {
    let sink = Arc::new(Collector::default());
    let (link, listener, password) = link(sink.clone()).await;
    let mut hub = FakeHub::accept(listener, &password).await;
    wait_bound(&link).await;

    let submitting =
        tokio::spawn(async move { link.submit(sms("7025550100", "911", "help")).await });
    hub.take_submission(CommandStatus::InvalidDestAddress).await;
    assert!(matches!(
        submitting.await.unwrap(),
        Err(InterconnectError::Rejected(_))
    ));
}

#[tokio::test]
async fn a_hub_that_is_busy_leaves_the_message_with_the_site() {
    let sink = Arc::new(Collector::default());
    let (link, listener, password) = link(sink.clone()).await;
    let mut hub = FakeHub::accept(listener, &password).await;
    wait_bound(&link).await;

    let submitting =
        tokio::spawn(async move { link.submit(sms("7025550100", "4155550118", "later")).await });
    hub.take_submission(CommandStatus::Throttled).await;
    assert!(matches!(
        submitting.await.unwrap(),
        Err(InterconnectError::Transient(_))
    ));
}

#[tokio::test]
async fn a_message_from_another_operator_lands_in_the_local_smsc() {
    let sink = Arc::new(Collector::default());
    let (link, listener, password) = link(sink.clone()).await;
    let mut hub = FakeHub::accept(listener, &password).await;
    wait_bound(&link).await;

    let status = hub
        .deliver("4155550118", "7025550100", "Hello 世界 €")
        .await;
    assert_eq!(status, CommandStatus::Ok);

    let taken = sink.taken.lock().unwrap();
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0].source, "4155550118");
    assert_eq!(taken[0].destination, "7025550100");
    assert_eq!(taken[0].text, "Hello 世界 €");
}

#[tokio::test]
async fn a_delivery_the_site_cannot_store_stays_with_the_hub() {
    let sink = Arc::new(Collector {
        refuse: true,
        ..Collector::default()
    });
    let (link, listener, password) = link(sink.clone()).await;
    let mut hub = FakeHub::accept(listener, &password).await;
    wait_bound(&link).await;

    let status = hub.deliver("4155550118", "7025550100", "keep it").await;
    assert_eq!(status, CommandStatus::TransientAppError);
    assert!(sink.taken.lock().unwrap().is_empty());
}

#[tokio::test]
async fn nothing_can_be_submitted_before_the_link_is_up() {
    let sink = Arc::new(Collector::default());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);

    let link = InterconnectLink::start(
        InterconnectConfig {
            role: BindRole::Client,
            bind_addr: None,
            hub_addr: Some(address),
            system_id: "kw6xyz-lv".into(),
            password: "s3cret08".into(),
        },
        sink,
    );
    assert!(!link.is_available());
}
