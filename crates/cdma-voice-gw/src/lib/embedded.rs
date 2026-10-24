use std::net::{Ipv4Addr, SocketAddr};

use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::TcpListenerStream;

use crate::proto::voice_gateway_server::VoiceGatewayServer;
use crate::service::VoiceGatewayService;

pub type Client = crate::proto::voice_gateway_client::VoiceGatewayClient<tonic::transport::Channel>;

pub type ControlStream = tonic::Streaming<crate::proto::GatewayToMscEvent>;
pub type MediaStream = tonic::Streaming<crate::proto::GatewayVoiceFrame>;

/// Await shutdown before stopping the Tokio runtime to release native SIP resources.
pub struct EmbeddedGateway {
    service: Option<VoiceGatewayService>,
    address: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    server: Option<JoinHandle<Result<(), tonic::transport::Error>>>,
}

impl EmbeddedGateway {
    pub async fn start(service: VoiceGatewayService) -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let (stop, stopped) = oneshot::channel();
        let retained_service = service.clone();
        let server = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(VoiceGatewayServer::new(service))
                .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                    let _ = stopped.await;
                })
                .await
        });
        Ok(Self {
            service: Some(retained_service),
            address,
            stop: Some(stop),
            server: Some(server),
        })
    }

    pub async fn client(&self) -> Result<Client, tonic::transport::Error> {
        Client::connect(format!("http://{}", self.address)).await
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub async fn shutdown(mut self) -> Result<(), EmbeddedError> {
        if let Some(service) = self.service.take() {
            service.shutdown().await?;
        }
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(mut server) = self.server.take() {
            const SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
            match tokio::time::timeout(SHUTDOWN_TIMEOUT, &mut server).await {
                Ok(result) => result??,
                Err(_) => {
                    server.abort();
                    return Err(EmbeddedError::ShutdownTimeout);
                }
            }
        }
        Ok(())
    }
}

impl Drop for EmbeddedGateway {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(server) = self.server.take() {
            server.abort();
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EmbeddedError {
    #[error("embedded voice gateway did not stop within its shutdown deadline")]
    ShutdownTimeout,
    #[error(transparent)]
    Backend(#[from] crate::sip::SipBackendError),
    #[error("embedded voice gateway task: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error("embedded voice gateway transport: {0}")]
    Transport(#[from] tonic::transport::Error),
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;
    use tokio_stream::wrappers::ReceiverStream;

    use crate::proto::msc_to_gateway_event::Event;
    use crate::{config::VoiceGatewayConfig, proto};

    use super::*;

    fn header<'a>(message: &'a str, name: &str) -> &'a str {
        message
            .lines()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                    .map(|(_, value)| value.trim())
            })
            .unwrap()
    }

    fn response(request: &str, address: SocketAddr, sdp: &str) -> String {
        format!(
            "SIP/2.0 200 OK\r\nVia: {}\r\nFrom: {}\r\nTo: {};tag=peer\r\nCall-ID: {}\r\nCSeq: {}\r\nContact: <sip:peer@{address}>\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{sdp}",
            header(request, "Via"),
            header(request, "From"),
            header(request, "To"),
            header(request, "Call-ID"),
            header(request, "CSeq"),
            sdp.len()
        )
    }

    async fn receive(socket: &tokio::net::UdpSocket, prefix: &str) -> (String, SocketAddr) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut bytes = [0u8; 8192];
            loop {
                let (length, source) = socket.recv_from(&mut bytes).await.unwrap();
                let message = String::from_utf8_lossy(&bytes[..length]).into_owned();
                if message.starts_with(prefix) {
                    return (message, source);
                }
            }
        })
        .await
        .unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn embedded_cdma_calls_clear_and_gateway_restarts_without_native_reference_reuse() {
        let _native = crate::sip::NATIVE_SIP_TEST_LOCK.lock().await;
        if !libre::native_available() {
            return;
        }
        for round in 0..2 {
            let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let address = peer.local_addr().unwrap();
            let listen = std::net::UdpSocket::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap();
            let rtp = std::net::UdpSocket::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap();
            let mut config = VoiceGatewayConfig::default();
            config.sip.listen_addr = listen.to_string();
            config.sip.request_uri_template = format!("sip:{{called}}@{address}");
            config.rtp.listen_addr = "127.0.0.1".into();
            config.rtp.port_range = [rtp.port(), rtp.port()];
            config.calls.max_concurrent_calls = 1;
            let service = VoiceGatewayService::try_new_with_libre(config).unwrap();
            let gateway = EmbeddedGateway::start(service).await.unwrap();
            let mut client = gateway.client().await.unwrap();
            let (control, input) = mpsc::channel(8);
            let mut events = client
                .control(ReceiverStream::new(input))
                .await
                .unwrap()
                .into_inner();
            let (media, input) = mpsc::channel(8);
            let frames = client
                .media(ReceiverStream::new(input))
                .await
                .unwrap()
                .into_inner();
            let sdp = format!(
                "v=0\r\nc=IN IP4 127.0.0.1\r\nm=audio {} RTP/AVP 0\r\n",
                rtp.port()
            );
            for call in 0..8 {
                let id = format!("cdma-{round}-{call}");
                control
                    .send(proto::MscToGatewayEvent {
                        event_id: String::new(),
                        event: Some(Event::StartOutboundCall(proto::StartOutboundCall {
                            session_id: id.clone(),
                            called_number: "2000".into(),
                            caller_number: "1001".into(),
                            service_option: 3,
                            ..Default::default()
                        })),
                    })
                    .await
                    .unwrap();
                let (invite, source) = receive(&peer, "INVITE ").await;
                peer.send_to(response(&invite, address, &sdp).as_bytes(), source)
                    .await
                    .unwrap();
                receive(&peer, "ACK ").await;
                let event =
                    tokio::time::timeout(std::time::Duration::from_secs(5), events.message())
                        .await
                        .unwrap()
                        .unwrap()
                        .unwrap();
                assert!(
                    matches!(
                        event.event.as_ref(),
                        Some(proto::gateway_to_msc_event::Event::Answered(answered)) if answered.session_id == id
                    ),
                    "unexpected control event: {event:?}"
                );
                let stats = client
                    .get_media_stats(proto::MediaStatsRequest {
                        session_ids: vec![id.clone()],
                    })
                    .await
                    .unwrap()
                    .into_inner();
                assert_eq!(stats.sessions.len(), 1);
                let stats = &stats.sessions[0];
                assert_eq!(stats.session_id, id);
                assert_eq!(stats.local_address, rtp.to_string());
                assert_eq!(stats.remote_address, rtp.to_string());
                assert_eq!(stats.rtp_rx_packets, 0);
                assert!(
                    client
                        .get_media_stats(proto::MediaStatsRequest {
                            session_ids: vec!["unrelated-session".into()],
                        })
                        .await
                        .unwrap()
                        .into_inner()
                        .sessions
                        .is_empty()
                );
                control
                    .send(proto::MscToGatewayEvent {
                        event_id: String::new(),
                        event: Some(Event::ReleaseCall(proto::ReleaseCall {
                            session_id: id,
                            reason: proto::ReleaseReason::MobileReleased as i32,
                        })),
                    })
                    .await
                    .unwrap();
                let (bye, source) = receive(&peer, "BYE ").await;
                peer.send_to(response(&bye, address, "").as_bytes(), source)
                    .await
                    .unwrap();
                let event =
                    tokio::time::timeout(std::time::Duration::from_secs(5), events.message())
                        .await
                        .unwrap()
                        .unwrap()
                        .unwrap();
                assert!(
                    matches!(
                        event.event,
                        Some(proto::gateway_to_msc_event::Event::Released(_))
                    ),
                    "unexpected release: {event:?}"
                );
            }
            drop(control);
            drop(media);
            drop(events);
            drop(frames);
            drop(client);
            gateway.shutdown().await.unwrap();
        }
    }
}
