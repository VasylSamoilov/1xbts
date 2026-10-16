//! UDP transport framing helpers for A11 signaling datagrams.

use std::net::SocketAddr;

use tokio::net::UdpSocket;

use crate::{
    AuthenticationVerifier, Error, Message, Result, UnverifiedDecodeReason, VerifiedMessage,
    decode_unverified, decode_verified, encode,
};

/// Fixed header size for the crate-local A11 UDP frame wrapper.
pub const UDP_FRAME_HEADER_LEN: usize = 2;

/// 3GPP2 A.S0017 carries A11 registration messages as bare Mobile-IP
/// registrations (RFC 3012 / RFC 2002) on UDP port 699, with no outer length
/// prefix. A real FAP/PCF binds this port and uses [`A11Framing::Bare`].
pub const A11_STANDARD_UDP_PORT: u16 = 699;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum A11Framing {
    /// Crate-local 2-byte big-endian payload-length prefix ahead of the
    /// message bytes.
    #[default]
    LengthPrefixed,
    /// Standards-conformant bare registration message with no outer prefix:
    /// the datagram is the whole A11 message. This is the framing a real
    /// FAP/PCF speaks on [`A11_STANDARD_UDP_PORT`].
    Bare,
}

/// Length-delimited UDP frame carrying one exact A11 message payload.
///
/// This helper is intentionally transport-local to the crate. It does not model
/// sockets, retries, or peer state. It only provides a reusable datagram format
/// for bundling one encoded A11 message with an explicit payload length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpFrame {
    /// Encoded A11 message carried by this frame.
    pub message: Message,
}

/// Length-delimited UDP frame carrying one verified A11 message payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedUdpFrame {
    /// Verified A11 message carried by this frame.
    pub message: VerifiedMessage,
}

impl UdpFrame {
    /// Builds a UDP frame from a typed A11 message.
    pub fn new(message: Message) -> Self {
        Self { message }
    }

    /// Returns the encoded message payload length in bytes.
    pub fn payload_len(&self) -> Result<u16> {
        let payload = encode(&self.message)?;
        u16::try_from(payload.len()).map_err(|_| Error::InvalidValue {
            context: "udp_frame.payload_len",
            reason: "encoded message exceeds u16 transport length",
        })
    }

    /// Encodes the UDP frame as `payload_length | message_bytes`.
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.encode_with(A11Framing::LengthPrefixed)
    }

    pub fn encode_with(&self, framing: A11Framing) -> Result<Vec<u8>> {
        let payload = encode(&self.message)?;
        match framing {
            A11Framing::Bare => Ok(payload),
            A11Framing::LengthPrefixed => {
                let payload_len =
                    u16::try_from(payload.len()).map_err(|_| Error::InvalidValue {
                        context: "udp_frame.payload_len",
                        reason: "encoded message exceeds u16 transport length",
                    })?;

                let mut out = Vec::with_capacity(UDP_FRAME_HEADER_LEN + payload.len());
                out.extend_from_slice(&payload_len.to_be_bytes());
                out.extend_from_slice(&payload);
                Ok(out)
            }
        }
    }

    pub fn decode_unverified(input: &[u8], reason: UnverifiedDecodeReason) -> Result<Self> {
        Self::decode_unverified_with(input, reason, A11Framing::LengthPrefixed)
    }

    pub fn decode_unverified_with(
        input: &[u8],
        reason: UnverifiedDecodeReason,
        framing: A11Framing,
    ) -> Result<Self> {
        let payload = frame_payload(input, framing)?;
        let message = decode_unverified(payload, reason)?;
        Ok(Self { message })
    }

    pub fn decode_verified<V>(input: &[u8], verifier: &V) -> Result<VerifiedUdpFrame>
    where
        V: AuthenticationVerifier + ?Sized,
    {
        Self::decode_verified_with(input, verifier, A11Framing::LengthPrefixed)
    }

    pub fn decode_verified_with<V>(
        input: &[u8],
        verifier: &V,
        framing: A11Framing,
    ) -> Result<VerifiedUdpFrame>
    where
        V: AuthenticationVerifier + ?Sized,
    {
        let payload = frame_payload(input, framing)?;
        let message = decode_verified(payload, verifier)?;
        Ok(VerifiedUdpFrame { message })
    }
}

/// Native UDP/IP endpoint for A11 signaling.
pub struct UdpEndpoint {
    socket: UdpSocket,
    framing: A11Framing,
}

impl UdpEndpoint {
    pub async fn bind(local_addr: SocketAddr) -> std::io::Result<Self> {
        Self::bind_with_framing(local_addr, A11Framing::LengthPrefixed).await
    }

    pub async fn bind_with_framing(
        local_addr: SocketAddr,
        framing: A11Framing,
    ) -> std::io::Result<Self> {
        UdpSocket::bind(local_addr)
            .await
            .map(|socket| Self { socket, framing })
    }

    pub fn framing(&self) -> A11Framing {
        self.framing
    }

    /// Returns the bound local address.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    pub async fn send_frame(&self, peer: SocketAddr, frame: &UdpFrame) -> std::io::Result<usize> {
        let bytes = frame
            .encode_with(self.framing)
            .map_err(to_io_invalid_data)?;
        self.socket.send_to(&bytes, peer).await
    }

    /// Sends one typed A11 message to a peer.
    pub async fn send_message(&self, peer: SocketAddr, message: Message) -> std::io::Result<usize> {
        self.send_frame(peer, &UdpFrame::new(message)).await
    }

    /// Receives and decodes one A11 UDP frame without verifying authentication.
    pub async fn recv_frame_unverified(
        &self,
        buf: &mut [u8],
        reason: UnverifiedDecodeReason,
    ) -> std::io::Result<(UdpFrame, SocketAddr)> {
        let (len, peer) = self.socket.recv_from(buf).await?;
        let frame = UdpFrame::decode_unverified_with(&buf[..len], reason, self.framing)
            .map_err(to_io_invalid_data)?;
        Ok((frame, peer))
    }

    /// Receives and decodes one typed A11 message without verifying authentication.
    pub async fn recv_message_unverified(
        &self,
        buf: &mut [u8],
        reason: UnverifiedDecodeReason,
    ) -> std::io::Result<(Message, SocketAddr)> {
        let (frame, peer) = self.recv_frame_unverified(buf, reason).await?;
        Ok((frame.message, peer))
    }

    /// Receives, decodes, and verifies one A11 UDP frame.
    pub async fn recv_frame_verified<V>(
        &self,
        buf: &mut [u8],
        verifier: &V,
    ) -> std::io::Result<(VerifiedUdpFrame, SocketAddr)>
    where
        V: AuthenticationVerifier + ?Sized,
    {
        let (len, peer) = self.socket.recv_from(buf).await?;
        let frame = UdpFrame::decode_verified_with(&buf[..len], verifier, self.framing)
            .map_err(to_io_invalid_data)?;
        Ok((frame, peer))
    }

    /// Receives, decodes, and verifies one typed A11 message.
    pub async fn recv_message_verified<V>(
        &self,
        buf: &mut [u8],
        verifier: &V,
    ) -> std::io::Result<(VerifiedMessage, SocketAddr)>
    where
        V: AuthenticationVerifier + ?Sized,
    {
        let (frame, peer) = self.recv_frame_verified(buf, verifier).await?;
        Ok((frame.message, peer))
    }
}

fn frame_payload(input: &[u8], framing: A11Framing) -> Result<&[u8]> {
    match framing {
        A11Framing::Bare => Ok(input),
        A11Framing::LengthPrefixed => udp_frame_payload(input),
    }
}

fn udp_frame_payload(input: &[u8]) -> Result<&[u8]> {
    if input.len() < UDP_FRAME_HEADER_LEN {
        return Err(Error::Truncated {
            needed: UDP_FRAME_HEADER_LEN,
            actual: input.len(),
        });
    }

    let payload_len = u16::from_be_bytes([input[0], input[1]]) as usize;
    let total_len = UDP_FRAME_HEADER_LEN + payload_len;
    if input.len() < total_len {
        return Err(Error::Truncated {
            needed: total_len,
            actual: input.len(),
        });
    }
    if input.len() != total_len {
        return Err(Error::InvalidValue {
            context: "udp_frame.length",
            reason: "frame contains trailing bytes beyond the declared payload",
        });
    }

    Ok(&input[UDP_FRAME_HEADER_LEN..total_len])
}

fn to_io_invalid_data(error: Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, error)
}
