use std::io;
use std::net::SocketAddr;

use tokio::net::UdpSocket;

use crate::message::{SipError, SipMessage};

const MAX_DATAGRAM: usize = 65_535;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("udp i/o error: {0}")]
    Io(#[from] io::Error),
    #[error("decode error: {0}")]
    Decode(#[from] SipError),
}

pub struct UdpSipTransport {
    socket: UdpSocket,
    buf: Vec<u8>,
}

impl UdpSipTransport {
    pub async fn bind(addr: SocketAddr) -> Result<Self, TransportError> {
        let socket = UdpSocket::bind(addr).await?;
        Ok(UdpSipTransport {
            socket,
            buf: vec![0u8; MAX_DATAGRAM],
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        Ok(self.socket.local_addr()?)
    }

    pub async fn recv(&mut self) -> Result<(SipMessage, SocketAddr), TransportError> {
        let (len, from) = self.socket.recv_from(&mut self.buf).await?;
        let msg = SipMessage::parse(&self.buf[..len])?;
        Ok((msg, from))
    }

    pub async fn send(&self, msg: &SipMessage, addr: SocketAddr) -> Result<usize, TransportError> {
        let bytes = msg.serialize();
        Ok(self.socket.send_to(&bytes, addr).await?)
    }
}
