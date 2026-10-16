//! CDMA CAVE authentication (C.S0005-E §2.3.12).
//!
//! The substitution table and round structure that make up the CAVE core are
//! export-controlled (TIA Common Cryptographic Algorithms / TSB-51) and not in
//! this repository, so core routines return [`CaveError::CoreUnavailable`].
//! Guessed table bytes would produce a plausible but wrong crypto core.

/// C.S0005-E §2.3.12.1: the authentication signature `AUTH_SIGNATURE` is 18 bits wide.
pub const AUTH_SIGNATURE_BITS: u32 = 18;

const AUTH_DATA_BITS: u32 = 24;
const AUTH_DATA_MASK: u32 = (1 << AUTH_DATA_BITS) - 1;

pub const AUTH_SIGNATURE_MASK: u32 = (1 << AUTH_SIGNATURE_BITS) - 1;

/// Bit width of `RANDSSD`, the random seed for SSD generation (C.S0005-E
/// §2.3.12.1.4). Stored as 7 bytes with the top byte carrying the high 8 bits.
pub const RANDSSD_BITS: u32 = 56;

/// Number of decimal digits packed into the global-challenge `AUTH_DATA` field
/// when a mobile originates a call (the last dialed digits, C.S0005-E
/// §2.3.12.1.3).
pub const DIALED_DIGITS: usize = 6;

/// Dialed digits pack 4 bits each. `1`–`9` encode as their binary value and `0` encodes as `1010` (ten).
const DIALED_ZERO_ENCODING: u8 = 0b1010;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CaveError {
    #[error(
        "CAVE core unavailable: the substitution table and round structure from TIA Common Cryptographic Algorithms / TSB-51 have not been supplied"
    )]
    CoreUnavailable,

    #[error("invalid dialed digit {0}: expected 0..=9")]
    InvalidDigit(u8),

    #[error("value 0x{value:x} does not fit in a {bits}-bit field")]
    FieldTooWide {
        /// The offending value.
        value: u32,
        /// The permitted field width in bits.
        bits: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AKey(pub [u8; 8]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Esn(pub u32);

/// The 56-bit `RANDSSD` seed, high 8 bits in byte 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RandSsd(pub [u8; 7]);

/// `ssd_a` keys the authentication signature. `ssd_b` keys CMEA and VPM derivation (C.S0005-E §2.3.12.1.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ssd {
    pub ssd_a: [u8; 8],
    pub ssd_b: [u8; 8],
}

/// C.S0005-E §2.3.12.1.3 selects `AUTH_DATA` by call type: an origination uses the last dialed digits, everything else uses `IMSI_S1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthData {
    DialedDigits([u8; DIALED_DIGITS]),
    ImsiS1(u32),
}

impl AuthData {
    pub fn to_u24(self) -> Result<u32, CaveError> {
        match self {
            AuthData::DialedDigits(digits) => pack_dialed_digits(&digits),
            AuthData::ImsiS1(v) => {
                if v > AUTH_DATA_MASK {
                    return Err(CaveError::FieldTooWide {
                        value: v,
                        bits: AUTH_DATA_BITS,
                    });
                }
                Ok(v)
            }
        }
    }
}

/// Most-significant digit first, 4 bits each. `1`–`9` encode as their binary value. `0` encodes as [`DIALED_ZERO_ENCODING`] (C.S0005-E §2.3.12.1.3).
pub fn pack_dialed_digits(digits: &[u8; DIALED_DIGITS]) -> Result<u32, CaveError> {
    let mut packed = 0u32;
    for &d in digits {
        if d > 9 {
            return Err(CaveError::InvalidDigit(d));
        }
        let nibble = if d == 0 { DIALED_ZERO_ENCODING } else { d };
        packed = (packed << 4) | u32::from(nibble);
    }
    Ok(packed)
}

/// SSD Generation Procedure (C.S0005-E §2.3.12.1.4).
pub fn generate_ssd(a_key: &AKey, esn: Esn, randssd: &RandSsd) -> Result<Ssd, CaveError> {
    let _ = (a_key, esn, randssd);
    cave_block()?;
    Err(CaveError::CoreUnavailable)
}

/// Auth Signature Procedure (C.S0005-E §2.3.12.1.1).
pub fn auth_signature(
    ssd_a: &[u8; 8],
    rand_challenge: u32,
    auth_data: u32,
    esn: Esn,
) -> Result<u32, CaveError> {
    if auth_data > AUTH_DATA_MASK {
        return Err(CaveError::FieldTooWide {
            value: auth_data,
            bits: AUTH_DATA_BITS,
        });
    }
    let _ = (ssd_a, rand_challenge, esn);
    cave_block()?;
    Err(CaveError::CoreUnavailable)
}

/// Global-challenge signature `AUTHR` (C.S0005-E §2.3.12.1.3).
pub fn authr(
    ssd_a: &[u8; 8],
    rand: u32,
    auth_data: AuthData,
    esn: Esn,
    imsi_s1: u32,
) -> Result<u32, CaveError> {
    let _ = imsi_s1;
    auth_signature(ssd_a, rand, auth_data.to_u24()?, esn)
}

/// Unique-challenge signature `AUTHU` (C.S0005-E §2.3.12.1.5).
///
/// Takes the already-assembled 32-bit `rand_challenge`: the `RANDU`-to-32-bit padding is part of the unsettled core and is not guessed here.
pub fn authu(
    ssd_a: &[u8; 8],
    rand_challenge: u32,
    esn: Esn,
    imsi_s1: u32,
) -> Result<u32, CaveError> {
    auth_signature(
        ssd_a,
        rand_challenge,
        AuthData::ImsiS1(imsi_s1).to_u24()?,
        esn,
    )
}

/// Base-station-challenge signature `AUTHBS` (C.S0005-E §2.3.12.1.6).
pub fn authbs(ssd_a: &[u8; 8], randbs: u32, esn: Esn, imsi_s1: u32) -> Result<u32, CaveError> {
    auth_signature(ssd_a, randbs, AuthData::ImsiS1(imsi_s1).to_u24()?, esn)
}

/// Length of the Voice Privacy Mask in bytes (520 bits, C.S0005-E §2.3.12.1.4).
pub const VPM_LEN: usize = 65;

/// The 64-bit CMEA signalling-message encryption key (C.S0005-E §2.3.12.1.4).
pub fn cmea_key(ssd_b: &[u8; 8], esn: Esn, rand: u32) -> Result<[u8; 8], CaveError> {
    let _ = (ssd_b, esn, rand);
    cave_block()?;
    Err(CaveError::CoreUnavailable)
}

/// The 520-bit Voice Privacy Mask from SSD-B (C.S0005-E §2.3.12.1.4).
pub fn vpm(ssd_b: &[u8; 8], esn: Esn, rand: u32) -> Result<[u8; VPM_LEN], CaveError> {
    let _ = (ssd_b, esn, rand);
    cave_block()?;
    Err(CaveError::CoreUnavailable)
}

/// Deliberately unimplemented: the substitution table and round structure are only in the export-controlled TIA Common Cryptographic Algorithms document (vectors in TSB-51), not in this repository. Guessed table bytes would produce a plausible but wrong crypto core.
fn cave_block() -> Result<(), CaveError> {
    Err(CaveError::CoreUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialed_digits_pack_msb_first_with_zero_as_ten() {
        let packed = pack_dialed_digits(&[1, 2, 3, 4, 5, 6]).unwrap();
        assert_eq!(packed, 0x123456);
        let with_zero = pack_dialed_digits(&[0, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(with_zero, 0xAAAAAA);
        assert_eq!(packed & !0x00FF_FFFF, 0);
    }

    #[test]
    fn dialed_digits_reject_out_of_range() {
        assert_eq!(
            pack_dialed_digits(&[1, 2, 3, 4, 5, 10]),
            Err(CaveError::InvalidDigit(10))
        );
    }

    #[test]
    fn auth_data_imsi_s1_width_checked() {
        assert_eq!(AuthData::ImsiS1(0x00AB_CDEF).to_u24().unwrap(), 0x00AB_CDEF);
        assert_eq!(
            AuthData::ImsiS1(0x0100_0000).to_u24(),
            Err(CaveError::FieldTooWide {
                value: 0x0100_0000,
                bits: 24,
            })
        );
    }

    #[test]
    fn auth_signature_width_constants() {
        assert_eq!(AUTH_SIGNATURE_BITS, 18);
        assert_eq!(AUTH_SIGNATURE_MASK, 0x3_FFFF);
        assert_eq!(AUTH_SIGNATURE_MASK.count_ones(), AUTH_SIGNATURE_BITS);
    }

    #[test]
    fn auth_signature_rejects_oversize_auth_data_before_core() {
        let err = auth_signature(&[0u8; 8], 0, 0x0100_0000, Esn(0)).unwrap_err();
        assert_eq!(
            err,
            CaveError::FieldTooWide {
                value: 0x0100_0000,
                bits: 24
            }
        );
    }

    #[test]
    fn core_dependent_routines_report_unavailable() {
        let a_key = AKey([0x11; 8]);
        let ssd_a = [0x22u8; 8];
        let ssd_b = [0x33u8; 8];
        assert_eq!(
            generate_ssd(&a_key, Esn(0x8000_1234), &RandSsd([0; 7])),
            Err(CaveError::CoreUnavailable)
        );
        assert_eq!(
            auth_signature(&ssd_a, 0x1234_5678, 0x00AB_CDEF, Esn(1)),
            Err(CaveError::CoreUnavailable)
        );
        assert_eq!(
            authr(
                &ssd_a,
                0x1234_5678,
                AuthData::ImsiS1(0x0012_3456),
                Esn(1),
                0x0012_3456
            ),
            Err(CaveError::CoreUnavailable)
        );
        assert_eq!(
            authu(&ssd_a, 0x1234_5678, Esn(1), 0x0012_3456),
            Err(CaveError::CoreUnavailable)
        );
        assert_eq!(
            authbs(&ssd_a, 0x1234_5678, Esn(1), 0x0012_3456),
            Err(CaveError::CoreUnavailable)
        );
        assert_eq!(cmea_key(&ssd_b, Esn(1), 0), Err(CaveError::CoreUnavailable));
        assert_eq!(vpm(&ssd_b, Esn(1), 0), Err(CaveError::CoreUnavailable));
    }

    #[test]
    #[ignore = "needs authoritative CAVE table + TSB-51 SSD-generation vector"]
    fn golden_generate_ssd() {
        let a_key = AKey([0; 8]); // TODO: authoritative A-key
        let esn = Esn(0); // TODO: authoritative ESN
        let randssd = RandSsd([0; 7]); // TODO: authoritative RANDSSD
        let ssd = generate_ssd(&a_key, esn, &randssd).expect("core must be present");
        assert_eq!(ssd.ssd_a, [0; 8], "TODO: expected SSD_A from vector");
        assert_eq!(ssd.ssd_b, [0; 8], "TODO: expected SSD_B from vector");
    }

    #[test]
    #[ignore = "needs authoritative CAVE table + TSB-51 Auth-Signature vector"]
    fn golden_authr() {
        let ssd_a = [0u8; 8]; // TODO: authoritative SSD_A
        let got = auth_signature(&ssd_a, 0, 0, Esn(0)).expect("core must be present");
        assert_eq!(got, 0, "TODO: expected AUTHR from vector");
        assert_eq!(got & !AUTH_SIGNATURE_MASK, 0, "signature must be 18 bits");
    }

    #[test]
    #[ignore = "self-consistency: runs once the CAVE core is present"]
    fn signature_is_deterministic_and_18_bits() {
        let ssd_a = [0x55u8; 8];
        let a = auth_signature(&ssd_a, 0x1111_2222, 0x00AA_BBCC, Esn(0x8000_0001))
            .expect("core must be present");
        let b = auth_signature(&ssd_a, 0x1111_2222, 0x00AA_BBCC, Esn(0x8000_0001))
            .expect("core must be present");
        assert_eq!(a, b, "same inputs must produce the same signature");
        assert_eq!(a & !AUTH_SIGNATURE_MASK, 0, "signature must be 18 bits");
    }

    #[test]
    #[ignore = "self-consistency: runs once the CAVE core is present"]
    fn one_input_bit_change_changes_signature() {
        let ssd_a = [0x55u8; 8];
        let base = auth_signature(&ssd_a, 0x1111_2222, 0x00AA_BBCC, Esn(0x8000_0001))
            .expect("core must be present");
        let flipped = auth_signature(&ssd_a, 0x1111_2223, 0x00AA_BBCC, Esn(0x8000_0001))
            .expect("core must be present");
        assert_ne!(
            base, flipped,
            "flipping a RAND bit must change the signature"
        );
    }
}
