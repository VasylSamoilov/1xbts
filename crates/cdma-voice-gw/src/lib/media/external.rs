//! Variable-duration speech frames use bounded 8 kHz PCM queues and 20 ms RTP playout.

use std::collections::{BTreeMap, VecDeque};

use crate::codec::{
    CodecError, MAX_FRAME_SAMPLES, RTP_FRAME_SAMPLES, SpeechCodec, SpeechCodecFactory,
};
use crate::proto::AirCodec;

use super::telephone_event::{EventReceiver, TelephoneEvent};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::time::{MissedTickBehavior, interval};

use super::{
    G711Codec, RtpMediaSession, Speech, build_rtp_packet, decode_g711_payload, encode_g711_frame,
    parse_rtp_packet,
};
use crate::proto::{GatewayVoiceFrame, VoiceFrameRate};
use crate::stun;

const RTP_PERIOD: Duration = Duration::from_millis(20);
const MAX_PACKET_SAMPLES: usize = MAX_FRAME_SAMPLES;
const MAX_DRAIN_PACKETS: usize = 32;

pub(super) struct ExternalSpeech {
    pub(super) codec: AirCodec,
    pub(super) samples: usize,
    encoder: Mutex<Box<dyn SpeechCodec>>,
    decoder: Mutex<Box<dyn SpeechCodec>>,
    outgoing: Mutex<VecDeque<i16>>,
}

impl ExternalSpeech {
    pub fn new(factory: &dyn SpeechCodecFactory) -> Result<Self, CodecError> {
        let codec = factory.air_codec();
        if !matches!(
            codec,
            AirCodec::IdenVselp8k | AirCodec::IdenVselp4k2 | AirCodec::Pcm168k
        ) {
            return Err(CodecError::Unsupported(codec as i32));
        }
        let encoder = factory.create()?;
        let decoder = factory.create()?;
        let samples = encoder.frame_samples();
        if samples == 0 || samples > MAX_FRAME_SAMPLES || decoder.frame_samples() != samples {
            return Err(CodecError::Frame(
                "invalid or inconsistent codec frame duration".into(),
            ));
        }
        Ok(Self {
            codec,
            samples,
            encoder: Mutex::new(encoder),
            decoder: Mutex::new(decoder),
            outgoing: Mutex::new(VecDeque::new()),
        })
    }

    pub fn queue(&self, bits: &[u8], rate_bps: u32) -> Result<(), CodecError> {
        if bits.iter().any(|bit| *bit > 1) {
            return Err(CodecError::Frame("speech bits must be 0 or 1".into()));
        }
        let pcm = if rate_bps == 0 {
            vec![0; self.samples]
        } else {
            self.decoder.lock().decode(bits, rate_bps)?
        };
        if pcm.len() != self.samples {
            return Err(CodecError::PcmLength {
                actual: pcm.len(),
                expected: self.samples,
            });
        }
        let mut outgoing = self.outgoing.lock();
        outgoing.extend(pcm);
        while outgoing.len() > MAX_FRAME_SAMPLES {
            outgoing.pop_front();
        }
        Ok(())
    }

    fn encode(&self, pcm: &[i16]) -> Result<crate::codec::EncodedSpeech, CodecError> {
        let frame = self.encoder.lock().encode(pcm)?;
        if frame.bits.is_empty() || frame.bits.iter().any(|bit| *bit > 1) {
            return Err(CodecError::Frame(
                "encoder returned invalid prioritized bits".into(),
            ));
        }
        Ok(frame)
    }
}

struct Playout {
    expected: Option<u32>,
    ssrc: Option<u32>,
    started: Option<Instant>,
    jitter: BTreeMap<u32, i16>,
    events: EventReceiver,
    incoming: VecDeque<i16>,
}

impl Playout {
    fn new() -> Self {
        Self {
            expected: None,
            ssrc: None,
            started: None,
            jitter: BTreeMap::new(),
            events: EventReceiver::default(),
            incoming: VecDeque::new(),
        }
    }

    fn packet(
        &mut self,
        session: &RtpMediaSession,
        bytes: &[u8],
        source: SocketAddr,
    ) -> Result<(), CodecError> {
        let packet = parse_rtp_packet(bytes).map_err(CodecError::Frame)?;
        let mut remote = session.remote_addr.lock();
        if remote.is_some_and(|remote| remote != source) {
            return Err(CodecError::Frame(
                "RTP source differs from negotiated peer".into(),
            ));
        }
        if self.ssrc.is_some_and(|ssrc| ssrc != packet.ssrc) {
            return Err(CodecError::Frame("RTP SSRC changed".into()));
        }
        let codec = G711Codec::from_payload_type(packet.payload_type);
        let event_payload = Some(packet.payload_type) == session.telephone_event_payload_type();
        if codec.is_none() && !event_payload {
            return Err(CodecError::Frame("unsupported RTP payload type".into()));
        }
        if packet.payload.len() > MAX_PACKET_SAMPLES {
            return Err(CodecError::Frame(
                "RTP payload exceeds the bounded media window".into(),
            ));
        }
        *remote = Some(source);
        drop(remote);
        self.ssrc = Some(packet.ssrc);
        let now = Instant::now();
        if self.expected.is_none()
            || packet
                .timestamp
                .wrapping_sub(self.expected.unwrap_or(packet.timestamp)) as i32
                > MAX_FRAME_SAMPLES as i32
        {
            self.expected = Some(packet.timestamp);
            self.started = Some(now + Duration::from_millis(session.jitter_buffer_ms));
            self.jitter.clear();
            self.incoming.clear();
        }
        let expected = self.expected.expect("initial packet sets timestamp");
        if event_payload {
            let event = TelephoneEvent::parse(packet.payload)
                .ok_or_else(|| CodecError::Frame("invalid telephone event".into()))?;
            if let Some(update) = self.events.receive(packet.ssrc, packet.timestamp, event) {
                for (index, sample) in update.pcm().into_iter().enumerate() {
                    let timestamp = packet
                        .timestamp
                        .wrapping_add(u32::from(update.start_sample))
                        .wrapping_add(index as u32);
                    if (0..MAX_FRAME_SAMPLES as i32)
                        .contains(&(timestamp.wrapping_sub(expected) as i32))
                    {
                        self.jitter.insert(timestamp, sample);
                    }
                }
            }
        } else if let Some(codec) = codec {
            for (index, sample) in decode_g711_payload(codec, packet.payload)
                .into_iter()
                .enumerate()
            {
                let timestamp = packet.timestamp.wrapping_add(index as u32);
                if (0..MAX_FRAME_SAMPLES as i32)
                    .contains(&(timestamp.wrapping_sub(expected) as i32))
                {
                    self.jitter.entry(timestamp).or_insert(sample);
                }
            }
        }
        session.rtp_rx_packets.fetch_add(1, Ordering::Relaxed);
        session.mark_activity();
        Ok(())
    }

    fn publish(
        &mut self,
        session: &RtpMediaSession,
        speech: &ExternalSpeech,
        now: Instant,
        skipped_samples: u32,
    ) -> Result<(), CodecError> {
        if self.started.is_none_or(|started| now < started) {
            return Ok(());
        }
        let Some(mut expected) = self.expected else {
            return Ok(());
        };
        if skipped_samples != 0 {
            expected = expected.wrapping_add(skipped_samples);
            self.incoming.clear();
        }
        for index in 0..RTP_FRAME_SAMPLES {
            self.incoming.push_back(
                self.jitter
                    .remove(&expected.wrapping_add(index as u32))
                    .unwrap_or(0),
            );
        }
        let next = expected.wrapping_add(RTP_FRAME_SAMPLES as u32);
        self.expected = Some(next);
        self.jitter
            .retain(|timestamp, _| timestamp.wrapping_sub(next) as i32 >= 0);
        while self.incoming.len() >= speech.samples {
            let pcm: Vec<_> = self.incoming.drain(..speech.samples).collect();
            let encoded = speech.encode(&pcm)?;
            let frame = GatewayVoiceFrame {
                session_id: session.session_id.clone(),
                num_bits: encoded.bits.len() as u32,
                bits: encoded.bits,
                rate: VoiceFrameRate::Unspecified as i32,
                sequence: session.recv_sequence.fetch_add(1, Ordering::Relaxed),
                service_option: 0,
                air_codec: speech.codec as i32,
                samples: speech.samples as u32,
                rate_bps: encoded.rate_bps,
            };
            if let Err(error) = session.gateway_voice_tx.send(frame) {
                log::trace!("VoiceGW media has no subscribers: {error}");
            }
        }
        Ok(())
    }
}

pub(super) async fn receive_loop(session: Arc<RtpMediaSession>) {
    let Speech::External(speech) = &session.speech else {
        return;
    };
    let mut playout = Playout::new();
    let mut bytes = [0u8; 2048];
    let mut tick = interval(RTP_PERIOD);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_tick = Instant::now();
    loop {
        tokio::select! {
            packet = session.socket.recv_from(&mut bytes) => {
                match packet {
                    Ok((length, source)) if !stun::is_stun_message(&bytes[..length]) => {
                        if let Err(error) = playout.packet(&session, &bytes[..length], source) {
                            session.rtp_rx_dropped_packets.fetch_add(1, Ordering::Relaxed);
                            log::debug!("VoiceGW external RTP packet dropped session={} error={error}", session.session_id);
                        }
                        for _ in 1..MAX_DRAIN_PACKETS {
                            match session.socket.try_recv_from(&mut bytes) {
                                Ok((length, source)) if !stun::is_stun_message(&bytes[..length]) => {
                                    if let Err(error) = playout.packet(&session, &bytes[..length], source) { log::debug!("VoiceGW external RTP packet dropped session={} error={error}", session.session_id); }
                                }
                                Ok(_) => {},
                                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                                Err(error) => { log::warn!("VoiceGW RTP receive failed: {error}"); return; }
                            }
                        }
                    }
                    Ok(_) => {},
                    Err(error) => { log::warn!("VoiceGW RTP receive failed: {error}"); return; }
                }
            }
            _ = tick.tick() => {
                let now = Instant::now();
                let elapsed_ticks = now.duration_since(last_tick).as_millis() / RTP_PERIOD.as_millis();
                let skipped = elapsed_ticks.saturating_sub(1).min(u128::from(u32::MAX / RTP_FRAME_SAMPLES as u32)) as u32 * RTP_FRAME_SAMPLES as u32;
                last_tick = now;
                if skipped != 0 {
                    let mut outgoing = speech.outgoing.lock();
                    for _ in 0..(skipped as usize).min(outgoing.len()) { outgoing.pop_front(); }
                    session.send_timestamp.fetch_add(skipped, Ordering::Relaxed);
                }
                let remote = *session.remote_addr.lock();
                if let Some(remote) = remote {
                    let codec = *session.codec.lock();
                    let pcm = {
                        let mut outgoing = speech.outgoing.lock();
                        std::array::from_fn(|_| outgoing.pop_front().unwrap_or(0))
                    };
                    let payload = encode_g711_frame(codec, &pcm);
                    let sequence = session.send_sequence.fetch_add(1, Ordering::Relaxed);
                    let timestamp = session.send_timestamp.fetch_add(RTP_FRAME_SAMPLES as u32, Ordering::Relaxed);
                    let packet = build_rtp_packet(codec.payload_type(), sequence, timestamp, session.ssrc, &payload);
                    if let Err(error) = session.socket.send_to(&packet, remote).await { log::warn!("VoiceGW RTP send failed: {error}"); return; }
                    session.rtp_tx_packets.fetch_add(1, Ordering::Relaxed);
                }
                if let Err(error) = playout.publish(&session, speech, now, skipped) { log::warn!("VoiceGW codec failed session={} error={error}", session.session_id); return; }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{linear_to_ulaw, ulaw_to_linear};
    use super::*;
    use tokio::net::UdpSocket;
    use tokio::sync::broadcast;
    use tokio::time::timeout;

    struct Factory<const N: usize>(Arc<Mutex<Vec<Vec<i16>>>>);
    struct Codec<const N: usize>(Arc<Mutex<Vec<Vec<i16>>>>);
    const SPEECH_SAMPLES: usize = 360;
    const DISPATCH_SAMPLES: usize = 720;

    impl<const N: usize> SpeechCodecFactory for Factory<N> {
        fn air_codec(&self) -> AirCodec {
            if N == RTP_FRAME_SAMPLES {
                AirCodec::Pcm168k
            } else if N == DISPATCH_SAMPLES {
                AirCodec::IdenVselp4k2
            } else {
                AirCodec::IdenVselp8k
            }
        }
        fn create(&self) -> Result<Box<dyn SpeechCodec>, CodecError> {
            Ok(Box::new(Codec::<N>(self.0.clone())))
        }
    }
    #[test]
    fn pcm_adapter_uses_twenty_millisecond_frames() {
        let factory = Factory::<RTP_FRAME_SAMPLES>(Default::default());
        let adapter = ExternalSpeech::new(&factory).unwrap();
        assert_eq!(adapter.codec, AirCodec::Pcm168k);
        assert_eq!(adapter.samples, RTP_FRAME_SAMPLES);
        let frame = adapter.encode(&[0; RTP_FRAME_SAMPLES]).unwrap();
        adapter.queue(&frame.bits, frame.rate_bps).unwrap();
        assert_eq!(adapter.outgoing.lock().len(), RTP_FRAME_SAMPLES);
    }
    impl<const N: usize> SpeechCodec for Codec<N> {
        fn frame_samples(&self) -> usize {
            N
        }
        fn encode(&mut self, pcm: &[i16]) -> Result<crate::codec::EncodedSpeech, CodecError> {
            self.0.lock().push(pcm.to_vec());
            Ok(crate::codec::EncodedSpeech {
                bits: vec![1; N],
                rate_bps: if N == DISPATCH_SAMPLES { 4200 } else { 8000 },
            })
        }
        fn decode(&mut self, _: &[u8], _: u32) -> Result<Vec<i16>, CodecError> {
            Ok(vec![1000; N])
        }
    }

    #[tokio::test]
    async fn dispatch_adapter_collects_ninety_ms_of_rtp_without_losing_samples() {
        let capture = Arc::new(Mutex::new(Vec::new()));
        let (frames, mut received) = broadcast::channel(8);
        let session = RtpMediaSession::bind_with_codec(
            "dispatch".into(),
            "127.0.0.1",
            0,
            G711Codec::Pcmu,
            &Factory::<DISPATCH_SAMPLES>(capture.clone()),
            0,
            false,
            false,
            frames,
            Some(101),
        )
        .await
        .unwrap();
        let mut playout = Playout::new();
        let source: SocketAddr = "127.0.0.1:1234".parse().unwrap();
        for index in 0..9u16 {
            let payload = vec![linear_to_ulaw(1000 + i16::try_from(index).unwrap() * 100); 80];
            playout
                .packet(
                    &session,
                    &build_rtp_packet(0, index, u32::from(index) * 80, 9, &payload),
                    source,
                )
                .unwrap();
        }
        let Speech::External(speech) = &session.speech else {
            panic!("expected external codec")
        };
        for _ in 0..5 {
            playout
                .publish(&session, speech, Instant::now(), 0)
                .unwrap();
        }
        let expected: Vec<_> = (0..9i16)
            .flat_map(|index| [ulaw_to_linear(linear_to_ulaw(1000 + index * 100)); 80])
            .collect();
        assert_eq!(capture.lock().as_slice(), &[expected]);
        let frame = received.try_recv().unwrap();
        assert_eq!(
            (frame.samples, frame.air_codec, frame.rate_bps),
            (DISPATCH_SAMPLES as u32, AirCodec::IdenVselp4k2 as i32, 4200)
        );
        assert!(received.try_recv().is_err());
    }

    #[tokio::test]
    async fn reordered_rtp_across_timestamp_wrap_produces_one_exact_45ms_frame() {
        let capture = Arc::new(Mutex::new(Vec::new()));
        let (frames, mut received) = broadcast::channel(8);
        let session = RtpMediaSession::bind_with_codec(
            "variable".into(),
            "127.0.0.1",
            0,
            G711Codec::Pcmu,
            &Factory::<SPEECH_SAMPLES>(capture.clone()),
            0,
            false,
            false,
            frames,
            Some(101),
        )
        .await
        .unwrap();
        let source: SocketAddr = "127.0.0.1:1234".parse().unwrap();
        let mut playout = Playout::new();
        let timestamp = u32::MAX - 200;
        let values = [1000, 2000, 3000, 4000, 5000, 6000];
        for index in [0, 2, 1, 3, 3, 5, 4] {
            let payload = vec![linear_to_ulaw(values[index]); 80];
            let packet = build_rtp_packet(
                0,
                index as u16,
                timestamp.wrapping_add(index as u32 * 80),
                9,
                &payload,
            );
            playout.packet(&session, &packet, source).unwrap();
        }
        let Speech::External(speech) = &session.speech else {
            panic!("expected external codec");
        };
        for _ in 0..3 {
            playout
                .publish(&session, speech, Instant::now(), 0)
                .unwrap();
        }
        let expected: Vec<_> = values
            .into_iter()
            .flat_map(|value| [ulaw_to_linear(linear_to_ulaw(value)); 80])
            .take(SPEECH_SAMPLES)
            .collect();
        assert_eq!(capture.lock().as_slice(), &[expected]);
        let frame = received.try_recv().unwrap();
        assert_eq!(
            (
                frame.samples,
                frame.air_codec,
                frame.rate_bps,
                frame.service_option
            ),
            (360, AirCodec::IdenVselp8k as i32, 8000, 0)
        );
        assert!(received.try_recv().is_err());
    }

    #[tokio::test]
    async fn one_45ms_speech_unit_is_sent_as_three_paced_20ms_rtp_packets() {
        let (frames, _) = broadcast::channel(8);
        let session = RtpMediaSession::bind_with_codec(
            "paced".into(),
            "127.0.0.1",
            0,
            G711Codec::Pcmu,
            &Factory::<SPEECH_SAMPLES>(Default::default()),
            0,
            false,
            false,
            frames,
            Some(101),
        )
        .await
        .unwrap();
        let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        *session.remote_addr.lock() = Some(peer.local_addr().unwrap());
        session
            .send_frame(&crate::proto::AirVoiceFrame {
                session_id: "paced".into(),
                bits: vec![1; 360],
                num_bits: 360,
                rate_bps: 8000,
                air_codec: AirCodec::IdenVselp8k as i32,
                samples: 360,
                ..Default::default()
            })
            .await
            .unwrap();
        let task = tokio::spawn(session.receive_loop());
        let mut bytes = [0u8; 2048];
        let mut recovered = Vec::new();
        let mut timestamps = Vec::new();
        for _ in 0..3 {
            let (length, _) = timeout(Duration::from_secs(1), peer.recv_from(&mut bytes))
                .await
                .unwrap()
                .unwrap();
            let packet = parse_rtp_packet(&bytes[..length]).unwrap();
            timestamps.push(packet.timestamp);
            recovered.extend_from_slice(packet.payload);
        }
        task.abort();
        assert_eq!(timestamps, [0, 160, 320]);
        assert_eq!(&recovered[..360], &[linear_to_ulaw(1000); 360]);
        assert_eq!(&recovered[360..], &[linear_to_ulaw(0); 120]);
    }
}
