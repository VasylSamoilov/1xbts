pub use sdr_hal::error::Error;
pub use sdr_hal::{Radio, RadioRx, RadioTx, RxReadResult, TxRadioHealth};

const NANOSECONDS_PER_SECOND: i128 = 1_000_000_000;
const TX_DELAY_REFERENCE_RATE_HZ: i128 = crate::consts::SR1_CHIP_RATE_HZ as i128 * 4;

pub fn tx_delay_samples_to_ns(delay_samples: i64) -> i64 {
    rounded_ratio(
        delay_samples as i128 * NANOSECONDS_PER_SECOND,
        TX_DELAY_REFERENCE_RATE_HZ,
    )
}

pub fn tx_delay_ns_to_samples(delay_ns: i64) -> i64 {
    rounded_ratio(
        delay_ns as i128 * TX_DELAY_REFERENCE_RATE_HZ,
        NANOSECONDS_PER_SECOND,
    )
}

fn rounded_ratio(value: i128, divisor: i128) -> i64 {
    ((value + value.signum() * (divisor / 2)) / divisor).clamp(i64::MIN as i128, i64::MAX as i128)
        as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tx_delay_conversion_preserves_signed_legacy_samples() {
        for delay_samples in [-100_000, -127, -70, -1, 0, 1, 70, 127, 100_000] {
            assert_eq!(
                tx_delay_ns_to_samples(tx_delay_samples_to_ns(delay_samples)),
                delay_samples
            );
        }
    }
}
