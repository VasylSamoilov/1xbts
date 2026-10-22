//! Reverse traffic frames contain 16 slots of 2048 chips, lasting 26.6667 ms.

mod ack_processor;
mod correlator;
mod data_ordering;
mod data_processor;
pub mod despread;
mod drc_processor;
pub mod finger;
mod rri_processor;
pub mod rri_subtype2;
pub mod subframe_harq;
pub mod subtype2_data;

pub use ack_processor::{HrpdReverseTrafficAckProcessor, TAG_ACK_PATTERN_PACKED};
pub use correlator::HrpdReverseTrafficCorrelator;
pub use data_processor::HrpdReverseTrafficDataProcessor;
pub use drc_processor::{DRC_SLOT_GATED_VALUE, HrpdReverseTrafficDrcProcessor, TAG_DRC_PACKED};
pub use finger::{
    HrpdReverseTrafficFinger, HrpdReverseTrafficFingerConfig, HrpdReverseTrafficFingerLock,
    TAG_DRC_COVER, TAG_DRC_LENGTH, TAG_FRAME_START_CHIP, TAG_MAC_INDEX, TAG_PILOT_COHERENCE_X1000,
    TAG_PILOT_SNR_DB_TENTHS, TAG_UATI,
};
pub use rri_processor::{
    HrpdReverseTrafficRriProcessor, TAG_RRI_MARGIN_DB_TENTHS, TAG_RRI_RATE_BPS,
    detect_hrpd_reverse_rri_rate,
};

#[cfg(test)]
mod tests;
