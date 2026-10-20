use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use cdma_interconnect::{InterconnectEgress, InterconnectError, InterconnectSms};
use smpp_core::tcp::{Connector, Endpoint};
use smpp_core::{
    Action, Address, CommandStatus, Config, Connection, ConnectionOptions, MessageBody,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};

use cdma_interconnect::{BindRole, InterconnectConfig};
use cdma_interconnect::{IncomingSms, MtSink};

const SUBMIT_QUEUE: usize = 64;

const RECONNECT_DELAY: Duration = Duration::from_secs(10);

enum Command {
    Submit {
        message: InterconnectSms,
        reply: oneshot::Sender<Result<(), InterconnectError>>,
    },
}

pub struct InterconnectLink {
    bound: Arc<AtomicBool>,
    commands: mpsc::Sender<Command>,
}

impl InterconnectLink {
    pub fn start(config: InterconnectConfig, sink: Arc<dyn MtSink>) -> Self {
        let (commands, rx) = mpsc::channel(SUBMIT_QUEUE);
        let bound = Arc::new(AtomicBool::new(false));
        tokio::spawn(run(config, sink, bound.clone(), rx));
        Self { bound, commands }
    }
}

#[async_trait]
impl InterconnectEgress for InterconnectLink {
    fn is_available(&self) -> bool {
        self.bound.load(Ordering::Relaxed)
    }

    async fn submit(&self, message: InterconnectSms) -> Result<(), InterconnectError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .try_send(Command::Submit { message, reply })
            .map_err(|_| InterconnectError::NotConnected)?;
        answer.await.unwrap_or(Err(InterconnectError::NotConnected))
    }
}

async fn run(
    config: InterconnectConfig,
    sink: Arc<dyn MtSink>,
    bound: Arc<AtomicBool>,
    mut commands: mpsc::Receiver<Command>,
) {
    let Some(address) = config.endpoint() else {
        return;
    };
    let mut connector = Connector::new(match config.role {
        BindRole::Client => Endpoint::Client(address),
        BindRole::Server => Endpoint::Server(address),
    });
    connector.reconnect_delay = RECONNECT_DELAY;
    while !commands.is_closed() {
        match connector.next(|| commands.is_closed()).await {
            Ok(Some(stream)) => {
                if let Err(err) = serve(&config, stream, &sink, &bound, &mut commands).await {
                    tracing::warn!("interconnect session ended: {err}");
                }
                bound.store(false, Ordering::Relaxed);
            }
            Ok(None) => break,
            Err(err) => tracing::debug!("interconnect not reachable: {err}"),
        }
    }
}

async fn serve<S>(
    config: &InterconnectConfig,
    stream: S,
    sink: &Arc<dyn MtSink>,
    bound: &Arc<AtomicBool>,
    commands: &mut mpsc::Receiver<Command>,
) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut session = Connection::new(
        stream,
        match config.role {
            BindRole::Client => Config::esme(config.system_id.clone(), config.password.clone()),
            BindRole::Server => Config::smsc(config.system_id.clone()),
        },
        ConnectionOptions::default(),
    );
    let mut outstanding: HashMap<u32, oneshot::Sender<Result<(), InterconnectError>>> =
        HashMap::new();
    let result = async {
        loop {
            tokio::select! {
                event = session.next() => {
                    let Some(action) = event.map_err(|e| e.to_string())? else { return Ok(()); };
                    match action {
                        Action::Send(pdu) => session.send(pdu).await.map_err(|e| e.to_string())?,
                        Action::Bound { .. } => {
                            tracing::info!("interconnect bound");
                            bound.store(true, Ordering::Relaxed);
                        }
                        Action::BindRequest { sequence, system_id, password } => {
                            let ok = system_id == config.system_id && password == config.password;
                            if ok {
                                session.accept_bind(sequence, system_id).await.map_err(|e| e.to_string())?;
                                bound.store(true, Ordering::Relaxed);
                            } else {
                                session.reject_bind(sequence, CommandStatus::BindFailed).await.map_err(|e| e.to_string())?;
                                return Ok(());
                            }
                        }
                        Action::Message { sequence, body } => {
                            let status = accept(sink, *body).await;
                            session.send(session.respond_to_message(sequence, status, String::new())).await.map_err(|e| e.to_string())?;
                        }
                        Action::MessageAck { sequence, status, .. } => {
                            if let Some(reply) = outstanding.remove(&sequence) { let _ = reply.send(answer(status)); }
                        }
                        Action::RequestTimeout { sequence } => {
                            if let Some(reply) = outstanding.remove(&sequence) {
                                let _ = reply.send(Err(InterconnectError::Transient("the hub did not answer in time".into())));
                            }
                        }
                        Action::Closed => return Ok(()),
                    }
                }
                command = commands.recv() => {
                    let Some(Command::Submit { message, reply }) = command else {
                        session.shutdown().await.map_err(|e| e.to_string())?;
                        return Ok(());
                    };
                    if reply.is_closed() { continue; }
                    if session.state() != smpp_core::State::Bound {
                        let _ = reply.send(Err(InterconnectError::NotConnected));
                        continue;
                    }
                    let (data_coding, text) = match smpp_core::text::encode(&message.text) {
                        Ok(encoded) => encoded,
                        Err(error) => {
                            let _ = reply.send(Err(InterconnectError::Rejected(error.to_string())));
                            continue;
                        }
                    };
                    let body = MessageBody {
                        source: Address::unknown(&message.source),
                        destination: Address::unknown(&message.destination),
                        data_coding,
                        text,
                        bearer_data: message.bearer_data.clone(),
                        teleservice: message.teleservice,
                        ..MessageBody::default()
                    };
                    match session.send_message(body).await {
                        Ok(sequence) => { outstanding.insert(sequence, reply); }
                        Err(err) => {
                            let _ = reply.send(Err(InterconnectError::Transient(err.to_string())));
                            if err.kind() != std::io::ErrorKind::WouldBlock { return Err(err.to_string()); }
                        }
                    }
                }
            }
        }
    }.await;

    for (_, reply) in outstanding {
        let _ = reply.send(Err(InterconnectError::Transient(
            "the interconnect session closed".into(),
        )));
    }
    result
}

async fn accept(sink: &Arc<dyn MtSink>, body: MessageBody) -> CommandStatus {
    let text = match smpp_core::text::decode(&body) {
        Ok(text) => text,
        Err(_) => return CommandStatus::InvalidMsgLength,
    };
    let message = IncomingSms {
        source: body.source.digits,
        destination: body.destination.digits,
        text,
        bearer_data: body.bearer_data,
        teleservice: body.teleservice,
    };
    let destination = message.destination.clone();
    match sink.deliver(message).await {
        Ok(sms_id) => {
            tracing::info!(%destination, %sms_id, "took a message from the interconnect");
            CommandStatus::Ok
        }
        Err(err) => {
            tracing::error!(%destination, "could not take a delivery: {err}");
            CommandStatus::TransientAppError
        }
    }
}

fn answer(status: CommandStatus) -> Result<(), InterconnectError> {
    match status {
        CommandStatus::Ok => Ok(()),
        CommandStatus::InvalidDestAddress
        | CommandStatus::InvalidSourceAddress
        | CommandStatus::InvalidMsgLength => {
            Err(InterconnectError::Rejected(format!("{status:?}")))
        }
        other => Err(InterconnectError::Transient(format!("{other:?}"))),
    }
}
