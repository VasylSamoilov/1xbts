//! Codec adapters exchange 8 kHz PCM and explicit air-interface frame durations.

use crate::proto::AirCodec;

pub const PCM_SAMPLE_RATE_HZ: u32 = 8000;
pub const RTP_FRAME_SAMPLES: usize = 160;
pub const MAX_FRAME_SAMPLES: usize = 1440;

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("unsupported air codec {0}")]
    Unsupported(i32),
    #[error("invalid speech frame: {0}")]
    Frame(String),
    #[error("invalid PCM frame length {actual}, expected {expected}")]
    PcmLength { actual: usize, expected: usize },
}

pub struct EncodedSpeech {
    /// One byte per codec bit, with values 0 or 1.
    pub bits: Vec<u8>,
    pub rate_bps: u32,
}

pub trait SpeechCodec: Send {
    /// PCM samples represented by one encoded frame at 8 kHz.
    fn frame_samples(&self) -> usize;
    fn encode(&mut self, pcm: &[i16]) -> Result<EncodedSpeech, CodecError>;
    fn decode(&mut self, bits: &[u8], rate_bps: u32) -> Result<Vec<i16>, CodecError>;
}

pub trait SpeechCodecFactory: Send + Sync {
    fn air_codec(&self) -> AirCodec;
    fn create(&self) -> Result<Box<dyn SpeechCodec>, CodecError>;
}

#[cfg(test)]
mod tests {
    use crate::proto::{AirCodec, StartOutboundCall};
    use prost::Message;

    #[test]
    fn existing_cdma_start_call_keeps_its_wire_encoding_and_defaults() {
        let bytes = [10, 1, b's', 18, 1, b'2', 26, 1, b'1', 32, 3];
        let call = StartOutboundCall::decode(bytes.as_slice()).unwrap();
        assert_eq!(call.air_codec, AirCodec::Unspecified as i32);
        assert_eq!(call.service_option, 3);
        assert_eq!(call.encode_to_vec(), bytes);
    }
}
