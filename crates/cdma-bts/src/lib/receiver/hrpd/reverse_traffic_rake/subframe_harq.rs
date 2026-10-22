//! Subtype-3 reverse traffic HARQ state machine (C.S0024-A §10.11 /
//! §13.2.1.3.11 / §13.3.1.3.2.2.4).
//!
//! One in-flight physical packet per reverse interlace. Each 4-slot
//! sub-frame's RRI detection routes the demapped data LLRs into the owning
//! interlace's accumulation buffer; a turbo decode + CRC-24 attempt runs
//! after every sub-packet so decodes terminate early. The outcome carries
//! the forward ARQ channel levels to schedule and, at most once per packet,
//! the CRC-valid physical-layer payload bits.

use num::complex::Complex32;

use crate::bts::hrpd::HrpdTransmissionMode;
use crate::bts::hrpd::harq_bus::ArqLevel;
use crate::phy::hrpd::turbo_decoder::HrpdTurboDecoder;
use cdma_common::hrpd::traffic::physical_crc24;

use super::rri_subtype2::{RriSubtype2Detection, is_rri_subtype2_null};
use super::subtype2_data::{
    MAX_SUBPACKETS, ModulationFormat, Subtype2DataFormat, accumulate_subpacket_llrs,
    demap_subpacket, mother_llrs_from_harq_buffer,
};

pub const REVERSE_INTERLACES: usize = 3;
const SUBPACKET_SPACING_SLOTS: u64 = REVERSE_INTERLACES as u64 * 4;
/// Slots from a sub-packet's first slot to its first ARQ response slot:
/// the sub-packet occupies m−8..m−5, responses go in m..m+2.
const SUBPACKET_ARQ_RESPONSE_OFFSET_SLOTS: u64 = 8;
/// Packet-level P-ARQ answers the whole packet four slots after the final
/// sub-packet's L-ARQ window: packet start +48..+50.
const PACKET_ARQ_RESPONSE_OFFSET_SLOTS: u64 = 12;
const ARQ_RESPONSE_SLOTS: u64 = 3;
/// Physical packet FCS + tail bits excluded from the CRC input.
const PACKET_FCS_BITS: usize = 24;
const PACKET_TAIL_BITS: usize = 6;
const B4_PHASE_MIN_SIGNIFICANCE_SQUARED: f64 = 16.0;
const B4_PHASE_MAX_CORRECTION_RAD: f32 = std::f32::consts::FRAC_PI_4;

#[derive(Clone)]
struct PacketState {
    format: &'static Subtype2DataFormat,
    harq: Vec<f32>,
    first_subpacket_start_slot: u64,
    last_subpacket_start_slot: u64,
    last_subpacket_id: u8,
    subpackets_accumulated: u8,
    decoded_payload: Option<Vec<u8>>,
    delivered: bool,
    terminal_reported: bool,
}

/// ARQ levels to schedule for one TX slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArqDecision {
    pub slot: u64,
    pub h_or_l: ArqLevel,
    pub p: ArqLevel,
}

#[derive(Clone, Default)]
pub struct SubframeOutcome {
    pub arq: Vec<ArqDecision>,
    /// CRC-valid reverse traffic MAC packet bits (physical packet minus
    /// FCS and tail), delivered exactly once per packet.
    pub delivered: Option<Vec<u8>>,
    pub decoded: bool,
    pub payload_bits: u32,
    pub subpacket_id: u8,
    pub interlace: u8,
    pub subpackets_accumulated: u8,
    pub first_subpacket_start_slot: Option<u64>,
    /// Zero means an earlier subpacket already decoded this packet.
    pub turbo_iterations: u8,
    pub llr_mean_abs: f32,
    pub mother_mean_abs: f32,
    pub terminal_packets: Vec<TerminalPacketOutcome>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalPacketDisposition {
    Decoded {
        transmission_mode: HrpdTransmissionMode,
    },
    Exhausted,
    Abandoned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalPacketOutcome {
    pub disposition: TerminalPacketDisposition,
    pub payload_bits: u32,
    pub first_subpacket_start_slot: u64,
    pub interlace: u8,
    pub subpackets_accumulated: u8,
    /// One-based subpacket number. Present only for a successful late/early decode.
    pub decoded_subpacket: Option<u8>,
}

#[derive(Clone)]
pub struct SubframeHarq {
    interlaces: [Option<PacketState>; REVERSE_INTERLACES],
    turbo_iterations: usize,
}

impl SubframeHarq {
    pub fn new() -> Self {
        Self {
            interlaces: [None, None, None],
            // A reverse sub-packet's H-ARQ answer is due only eight slots
            // after its start. CRC early termination handles clean packets;
            // cap a marginal attempt so it cannot stall the RX worker past
            // several subsequent ARQ deadlines. A later redundancy
            // sub-packet gets another decode attempt with combined LLRs.
            turbo_iterations: 4,
        }
    }

    /// Ingest one sub-frame's RRI detection and demodulated data symbols.
    ///
    /// `start_slot` is the sub-frame's first slot in absolute system time;
    /// `frame_offset` is the assignment FrameOffset. `w24`/`w12` are the
    /// derotated Walsh-decovered data-branch symbols for the sub-frame.
    pub fn ingest_subframe(
        &mut self,
        start_slot: u64,
        frame_offset: u8,
        rri: &RriSubtype2Detection,
        w24: &[Complex32],
        w12: &[Complex32],
    ) -> SubframeOutcome {
        let mut outcome = SubframeOutcome {
            payload_bits: rri.payload_bits,
            subpacket_id: rri.subpacket_id,
            ..Default::default()
        };
        let interlace = reverse_interlace_offset(start_slot, frame_offset);
        outcome.interlace = interlace;
        let slot_of_interlace = &mut self.interlaces[usize::from(interlace)];

        if is_rri_subtype2_null(rri) {
            if let Some(state) = slot_of_interlace.take()
                && !state.terminal_reported
            {
                outcome.terminal_packets.push(terminal_packet(
                    &state,
                    interlace,
                    TerminalPacketDisposition::Abandoned,
                    None,
                ));
            }
            return outcome;
        }
        let Some(format) = Subtype2DataFormat::for_payload_bits(rri.payload_bits as usize) else {
            return outcome;
        };

        let starts_new_packet = match slot_of_interlace.as_ref() {
            None => true,
            Some(state) => {
                let subpacket_advance = rri.subpacket_id.saturating_sub(state.last_subpacket_id);
                let expected_slot = state
                    .last_subpacket_start_slot
                    .checked_add(u64::from(subpacket_advance) * SUBPACKET_SPACING_SLOTS);
                let expected_continuation = std::ptr::eq(state.format, format)
                    && subpacket_advance > 0
                    && expected_slot == Some(start_slot)
                    && usize::from(rri.subpacket_id) < MAX_SUBPACKETS;
                !expected_continuation
            }
        };
        if starts_new_packet {
            if let Some(state) = slot_of_interlace.take()
                && !state.terminal_reported
            {
                outcome.terminal_packets.push(terminal_packet(
                    &state,
                    interlace,
                    TerminalPacketDisposition::Abandoned,
                    None,
                ));
            }
            *slot_of_interlace = Some(PacketState {
                format,
                harq: format.new_harq_buffer(),
                first_subpacket_start_slot: start_slot,
                last_subpacket_start_slot: start_slot,
                last_subpacket_id: rri.subpacket_id,
                subpackets_accumulated: 0,
                decoded_payload: None,
                delivered: false,
                terminal_reported: false,
            });
        }
        let state = slot_of_interlace.as_mut().expect("state just ensured");

        outcome.first_subpacket_start_slot = Some(state.first_subpacket_start_slot);
        state.last_subpacket_start_slot = start_slot;
        state.last_subpacket_id = rri.subpacket_id;
        state.subpackets_accumulated = state.subpackets_accumulated.saturating_add(1);
        outcome.subpackets_accumulated = state.subpackets_accumulated;

        if state.decoded_payload.is_none() {
            let llrs = demap_subpacket(format, w24, w12);
            outcome.llr_mean_abs = mean_abs(&llrs);
            let rri_harq_subpacket = usize::from(rri.subpacket_id.min(MAX_SUBPACKETS as u8 - 1));
            accumulate_subpacket_llrs(format, &mut state.harq, rri_harq_subpacket, &llrs);

            // The slot timing fixes the descramble interlace and the RRI fixes
            // the sub-packet identifier, so decode against exactly those; CRC-24
            // confirms delivery.
            let first =
                decode_harq_candidate(format, &state.harq, interlace, self.turbo_iterations);
            outcome.mother_mean_abs = first.mother_mean_abs;
            outcome.turbo_iterations = first.iterations_used as u8;
            if let Some(payload) = first.decoded_payload {
                state.decoded_payload = Some(payload);
            } else if matches!(format.modulation, ModulationFormat::B4)
                && let Some(rotor) = b4_residual_phase_rotor(w24)
            {
                let corrected: Vec<_> = w24.iter().map(|symbol| *symbol * rotor).collect();
                let mut delta = demap_subpacket(format, &corrected, w12);
                for (value, original) in delta.iter_mut().zip(&llrs) {
                    *value -= original;
                }
                let mut candidate = state.harq.clone();
                accumulate_subpacket_llrs(format, &mut candidate, rri_harq_subpacket, &delta);
                let retry =
                    decode_harq_candidate(format, &candidate, interlace, self.turbo_iterations);
                outcome.turbo_iterations = outcome
                    .turbo_iterations
                    .saturating_add(retry.iterations_used as u8);
                if let Some(payload) = retry.decoded_payload {
                    log::debug!(
                        "hrpd_reverse_b4_phase_recovered: start_slot={} payload_bits={} correction_rad={:.3} iterations={}",
                        start_slot,
                        format.payload_bits,
                        -rotor.arg(),
                        outcome.turbo_iterations
                    );
                    state.harq = candidate;
                    state.decoded_payload = Some(payload);
                    outcome.mother_mean_abs = retry.mother_mean_abs;
                }
            }
        }
        outcome.decoded = state.decoded_payload.is_some();
        if outcome.decoded && !state.delivered {
            state.delivered = true;
            outcome.delivered = state.decoded_payload.clone();
        }
        if outcome.decoded && !state.terminal_reported {
            if let Some(transmission_mode) = state
                .decoded_payload
                .as_ref()
                .and_then(|payload| payload.last())
                .map(|mode| HrpdTransmissionMode::from_low_latency(mode & 1 != 0))
            {
                state.terminal_reported = true;
                outcome.terminal_packets.push(terminal_packet(
                    state,
                    interlace,
                    TerminalPacketDisposition::Decoded { transmission_mode },
                    Some(rri.subpacket_id.saturating_add(1)),
                ));
            }
        }

        let final_subpacket = rri.subpacket_id as usize + 1 >= MAX_SUBPACKETS;
        let (h_or_l, p) = if final_subpacket {
            // L-ARQ (NAK-oriented OOK) replaces H-ARQ after the final
            // sub-packet; P-ARQ answers the whole packet on the opposite
            // phase. Both NAK a failed final sub-packet and stay silent on a
            // decode.
            let level = if outcome.decoded {
                ArqLevel::Off
            } else {
                ArqLevel::Minus
            };
            (level, level)
        } else {
            let h = if outcome.decoded {
                ArqLevel::Plus
            } else {
                ArqLevel::Minus
            };
            (h, ArqLevel::Off)
        };

        push_arq_window(
            &mut outcome.arq,
            start_slot + SUBPACKET_ARQ_RESPONSE_OFFSET_SLOTS,
            frame_offset,
            h_or_l,
            ArqLevel::Off,
        );
        if final_subpacket {
            push_arq_window(
                &mut outcome.arq,
                start_slot + PACKET_ARQ_RESPONSE_OFFSET_SLOTS,
                frame_offset,
                ArqLevel::Off,
                p,
            );
        }
        if final_subpacket && !outcome.decoded {
            state.terminal_reported = true;
            outcome.terminal_packets.push(terminal_packet(
                state,
                interlace,
                TerminalPacketDisposition::Exhausted,
                None,
            ));
            // Packet exhausted its sub-packets; free the interlace.
            *slot_of_interlace = None;
        }
        outcome
    }
}

fn terminal_packet(
    state: &PacketState,
    interlace: u8,
    disposition: TerminalPacketDisposition,
    decoded_subpacket: Option<u8>,
) -> TerminalPacketOutcome {
    TerminalPacketOutcome {
        disposition,
        payload_bits: state.format.payload_bits as u32,
        first_subpacket_start_slot: state.first_subpacket_start_slot,
        interlace,
        subpackets_accumulated: state.subpackets_accumulated,
        decoded_subpacket,
    }
}

fn push_arq_window(
    arq: &mut Vec<ArqDecision>,
    response_start: u64,
    frame_offset: u8,
    h_or_l: ArqLevel,
    p: ArqLevel,
) {
    for slot in response_start..response_start + ARQ_RESPONSE_SLOTS {
        // RPC/DRCLock own the (T - FrameOffset) mod 4 = 3 slots.
        if (slot + 4 - u64::from(frame_offset & 0x03)) % 4 == 3 {
            continue;
        }
        arq.push(ArqDecision { slot, h_or_l, p });
    }
}

fn mean_abs(values: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().map(|v| v.abs()).sum::<f32>() / values.len() as f32
}

struct DecodeHarqCandidate {
    mother_mean_abs: f32,
    decoded_payload: Option<Vec<u8>>,
    iterations_used: usize,
}

fn b4_residual_phase_rotor(symbols: &[Complex32]) -> Option<Complex32> {
    if symbols.len() < 2 {
        return None;
    }
    let mut axis = Complex32::new(0.0, 0.0);
    let mut fourth_moment_sum = 0.0f64;
    for symbol in symbols {
        // B4 is antipodal on Q, so negating its square removes the data sign.
        axis -= *symbol * *symbol;
        fourth_moment_sum += f64::from(symbol.norm_sqr()).powi(2);
    }
    let axis_power = f64::from(axis.norm_sqr());
    let variance = (fourth_moment_sum - axis_power / symbols.len() as f64).max(0.0);
    let phase = axis.arg() / 2.0;
    if !phase.is_finite()
        || axis_power <= B4_PHASE_MIN_SIGNIFICANCE_SQUARED * variance
        || phase.abs() >= B4_PHASE_MAX_CORRECTION_RAD
        || phase == 0.0
    {
        return None;
    }
    Some(Complex32::from_polar(1.0, -phase))
}

fn decode_harq_candidate(
    format: &'static Subtype2DataFormat,
    harq: &[f32],
    interlace: u8,
    turbo_iterations: usize,
) -> DecodeHarqCandidate {
    let mother = mother_llrs_from_harq_buffer(format, harq, interlace);
    let mother_mean_abs = mean_abs(&mother);
    let decoded = HrpdTurboDecoder::new(format.payload_bits as u32).and_then(|decoder| {
        decoder
            .with_iterations(turbo_iterations)
            .decode_until(&mother, packet_crc24_ok)
    });
    let iterations_used = decoded
        .as_ref()
        .map_or(turbo_iterations, |(_, iterations)| *iterations);
    let decoded_payload = decoded.map(|(bits, _)| {
        let mac_end = bits.len() - PACKET_FCS_BITS - PACKET_TAIL_BITS;
        bits[..mac_end].to_vec()
    });
    DecodeHarqCandidate {
        mother_mean_abs,
        decoded_payload,
        iterations_used,
    }
}

impl Default for SubframeHarq {
    fn default() -> Self {
        Self::new()
    }
}

/// Reverse-link interlace for a sub-packet starting at `slot`
/// (C.S0024-A footnote 112).
pub fn reverse_interlace_offset(slot: u64, frame_offset: u8) -> u8 {
    (((slot.saturating_sub(u64::from(frame_offset))) / 4) % 3) as u8
}

pub(crate) fn packet_crc24_ok(bits: &[u8]) -> bool {
    if bits.len() <= PACKET_FCS_BITS + PACKET_TAIL_BITS {
        return false;
    }
    let mac_end = bits.len() - PACKET_FCS_BITS - PACKET_TAIL_BITS;
    let expected = physical_crc24(&bits[..mac_end]);
    let mut observed = 0u32;
    for &bit in &bits[mac_end..mac_end + PACKET_FCS_BITS] {
        observed = (observed << 1) | u32::from(bit & 1);
    }
    expected == observed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phy::hrpd::turbo::HrpdTurboEncoder;
    use crate::receiver::hrpd::reverse_traffic_rake::rri_subtype2::RRI_SUBTYPE2_PAYLOAD_BITS;
    use crate::receiver::hrpd::reverse_traffic_rake::subtype2_data::{
        decover_w12_symbols, decover_w24_symbols, subpacket_code_symbols,
    };
    use cdma_common::hrpd::traffic::physical_crc24;

    fn lcg_bits(count: usize, seed: u32) -> Vec<u8> {
        let mut s = seed;
        (0..count)
            .map(|_| {
                s = s.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                ((s >> 16) & 1) as u8
            })
            .collect()
    }

    fn build_packet_bits_with_mode(
        payload_bits: usize,
        seed: u32,
        transmission_mode: HrpdTransmissionMode,
    ) -> Vec<u8> {
        let mac_bits = payload_bits - PACKET_FCS_BITS - PACKET_TAIL_BITS;
        let mut bits = lcg_bits(mac_bits, seed);
        *bits.last_mut().expect("MAC trailer") =
            u8::from(transmission_mode == HrpdTransmissionMode::LowLatency);
        let fcs = physical_crc24(&bits);
        for i in (0..PACKET_FCS_BITS).rev() {
            bits.push(((fcs >> i) & 1) as u8);
        }
        bits.extend(std::iter::repeat_n(0u8, PACKET_TAIL_BITS));
        bits
    }

    fn build_packet_bits(payload_bits: usize, seed: u32) -> Vec<u8> {
        build_packet_bits_with_mode(payload_bits, seed, HrpdTransmissionMode::HighCapacity)
    }

    fn tx_subframe_symbols(
        format: &'static Subtype2DataFormat,
        packet_bits: &[u8],
        subpacket: usize,
        interlace: u8,
    ) -> (Vec<Complex32>, Vec<Complex32>) {
        let encoder = HrpdTurboEncoder::new(format.payload_bits as u32).expect("encoder");
        let (num, den) = (1, format.turbo_code_rate_den);
        let mut coded = encoder.encode(packet_bits, num, den);
        format.scramble_encoder_output(&mut coded, interlace);
        let interleaved = format.interleave_encoder_output(&coded);
        let code_symbols = subpacket_code_symbols(format, &interleaved, subpacket);
        let chips = format.modulate_subpacket(&code_symbols);
        let w24 = if format.subframe_w24_symbols() > 0 {
            decover_w24_symbols(&chips)
        } else {
            Vec::new()
        };
        let w12 = if format.subframe_w12_symbols() > 0 {
            decover_w12_symbols(&chips)
        } else {
            Vec::new()
        };
        (w24, w12)
    }

    fn detection(payload_bits: u32, subpacket_id: u8) -> RriSubtype2Detection {
        let payload_index = RRI_SUBTYPE2_PAYLOAD_BITS
            .iter()
            .position(|&b| b == payload_bits)
            .expect("payload in RRI table") as u8;
        RriSubtype2Detection {
            payload_index,
            subpacket_id,
            payload_bits,
            best_score: 1.0,
            second_score: 0.0,
            margin: 1.0,
        }
    }

    #[test]
    fn b4_phase_retry_failure_preserves_combining_buffer() {
        let format = Subtype2DataFormat::for_payload_bits(512).unwrap();
        let rotor = Complex32::from_polar(1.0, 0.4);
        let symbols: Vec<_> = lcg_bits(format.subframe_w24_symbols(), 0x1278)
            .into_iter()
            .map(|bit| rotor * Complex32::new(0.0, if bit == 0 { 1.0 } else { -1.0 }))
            .collect();
        let mut expected = format.new_harq_buffer();
        accumulate_subpacket_llrs(
            format,
            &mut expected,
            0,
            &demap_subpacket(format, &symbols, &[]),
        );
        let mut harq = SubframeHarq::new();
        let outcome = harq.ingest_subframe(120, 0, &detection(512, 0), &symbols, &[]);
        assert!(!outcome.decoded);
        assert!(outcome.delivered.is_none());
        assert_eq!(outcome.turbo_iterations, 8);
        let state = harq.interlaces[0].as_ref().unwrap();
        assert_eq!(state.harq, expected);
        assert_eq!(state.subpackets_accumulated, 1);
    }

    #[test]
    fn b4_residual_phase_retains_pilot_quadrant_and_rejects_isotropic_energy() {
        for phase in [-0.6, 0.6] {
            let symbols: Vec<_> = lcg_bits(512, 0x1991)
                .into_iter()
                .map(|bit| {
                    Complex32::from_polar(1.0, phase)
                        * Complex32::new(0.0, if bit == 0 { 1.0 } else { -1.0 })
                })
                .collect();
            let rotor = b4_residual_phase_rotor(&symbols).unwrap();
            for symbol in symbols {
                assert!((symbol * rotor).re.abs() < 1.0e-5);
            }
        }
        let isotropic = [Complex32::new(1.0, 0.0), Complex32::new(0.0, 1.0)].repeat(256);
        assert!(b4_residual_phase_rotor(&isotropic).is_none());
        assert!(b4_residual_phase_rotor(&vec![Complex32::new(1.0, 0.0); 512]).is_none());
        assert!(b4_residual_phase_rotor(&vec![Complex32::new(0.0, 0.0); 512]).is_none());
    }

    #[test]
    fn early_termination_delivers_once_and_acks() {
        let format = Subtype2DataFormat::for_payload_bits(1024).expect("format");
        let packet = build_packet_bits(1024, 0xA5A5_0001);
        let start_slot = 120u64;
        let (w24, w12) = tx_subframe_symbols(format, &packet, 0, 0);

        let mut harq = SubframeHarq::new();
        let outcome = harq.ingest_subframe(start_slot, 0, &detection(1024, 0), &w24, &w12);

        assert!(outcome.decoded, "clean sub-packet 0 decodes immediately");
        assert_eq!(outcome.turbo_iterations, 1);
        let mac_end = packet.len() - PACKET_FCS_BITS - PACKET_TAIL_BITS;
        assert_eq!(outcome.delivered.as_deref(), Some(&packet[..mac_end]));
        assert_eq!(
            outcome.terminal_packets,
            vec![TerminalPacketOutcome {
                disposition: TerminalPacketDisposition::Decoded {
                    transmission_mode: HrpdTransmissionMode::HighCapacity,
                },
                payload_bits: 1024,
                first_subpacket_start_slot: start_slot,
                interlace: 0,
                subpackets_accumulated: 1,
                decoded_subpacket: Some(1),
            }]
        );
        // H-ARQ ACK slots m..m+2 with m = start + 8, minus the RPC slot
        // ((slot − 0) mod 4 == 3 → slot 131 excluded).
        let acked: Vec<u64> = outcome.arq.iter().map(|d| d.slot).collect();
        assert_eq!(acked, vec![128, 129, 130]);
        assert!(
            outcome
                .arq
                .iter()
                .all(|d| d.h_or_l == ArqLevel::Plus && d.p == ArqLevel::Off)
        );

        // The AT continues the packet anyway (missed ACK): no double
        // delivery, ACK re-published.
        let (w24b, w12b) = tx_subframe_symbols(format, &packet, 1, 0);
        let again = harq.ingest_subframe(start_slot + 12, 0, &detection(1024, 1), &w24b, &w12b);
        assert!(again.decoded);
        assert!(again.delivered.is_none(), "payload delivered exactly once");
        assert!(
            again.terminal_packets.is_empty(),
            "terminal outcome emitted exactly once"
        );
    }

    #[test]
    fn missed_subpacket_preserves_combining_and_single_delivery() {
        let format = Subtype2DataFormat::for_payload_bits(1024).expect("format");
        let packet = build_packet_bits(1024, 0xA5A5_0012);
        let mac_end = packet.len() - PACKET_FCS_BITS - PACKET_TAIL_BITS;
        let start_slot = 120;
        for first_decodes in [false, true] {
            let (mut first_w24, first_w12) = tx_subframe_symbols(format, &packet, 0, 0);
            if !first_decodes {
                for symbol in &mut first_w24 {
                    *symbol *= -0.01;
                }
            }
            let mut harq = SubframeHarq::new();
            let first =
                harq.ingest_subframe(start_slot, 0, &detection(1024, 0), &first_w24, &first_w12);
            assert_eq!(first.decoded, first_decodes);
            let (third_w24, third_w12) = tx_subframe_symbols(format, &packet, 2, 0);
            let third = harq.ingest_subframe(
                start_slot + 2 * SUBPACKET_SPACING_SLOTS,
                0,
                &detection(1024, 2),
                &third_w24,
                &third_w12,
            );
            assert!(third.decoded);
            assert_eq!(third.subpackets_accumulated, 2);
            assert!(
                !third
                    .terminal_packets
                    .iter()
                    .any(|terminal| terminal.disposition == TerminalPacketDisposition::Abandoned)
            );
            let delivered = [first.delivered, third.delivered]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            assert_eq!(delivered, vec![packet[..mac_end].to_vec()]);
        }
    }

    #[test]
    fn max_payload_decodes_first_subpacket_in_one_iteration() {
        let format = Subtype2DataFormat::for_payload_bits(12_288).expect("format");
        let packet =
            build_packet_bits_with_mode(12_288, 0xA5A5_1228, HrpdTransmissionMode::LowLatency);
        let start_slot = 120u64;
        let (w24, w12) = tx_subframe_symbols(format, &packet, 0, 0);

        let mut harq = SubframeHarq::new();
        let started = std::time::Instant::now();
        let outcome = harq.ingest_subframe(start_slot, 0, &detection(12_288, 0), &w24, &w12);

        assert!(outcome.decoded);
        assert_eq!(outcome.turbo_iterations, 1);
        assert!(matches!(
            outcome.terminal_packets.as_slice(),
            [TerminalPacketOutcome {
                disposition: TerminalPacketDisposition::Decoded {
                    transmission_mode: HrpdTransmissionMode::LowLatency
                },
                ..
            }]
        ));
        eprintln!(
            "12288-bit first-subpacket decode elapsed_us={}",
            started.elapsed().as_micros()
        );
    }

    #[test]
    fn failed_packet_naks_and_frees_interlace_after_final_subpacket() {
        let format = Subtype2DataFormat::for_payload_bits(1024).expect("format");
        let packet = build_packet_bits(1024, 0x5A5A_0002);
        let (w24, w12) = tx_subframe_symbols(format, &packet, 0, 0);
        // Corrupt the symbols beyond decodability.
        let w24: Vec<Complex32> = w24.iter().map(|c| -c * 0.01).collect();
        let w12: Vec<Complex32> = w12.iter().map(|c| -c * 0.01).collect();

        let mut harq = SubframeHarq::new();
        for (sp, base) in [(0u8, 120u64), (1, 132), (2, 144), (3, 156)] {
            let outcome = harq.ingest_subframe(base, 0, &detection(1024, sp), &w24, &w12);
            assert!(!outcome.decoded);
            if sp < 3 {
                assert!(outcome.terminal_packets.is_empty());
                assert!(
                    outcome
                        .arq
                        .iter()
                        .all(|d| d.h_or_l == ArqLevel::Minus && d.p == ArqLevel::Off),
                    "H-ARQ NAK while sub-packets remain"
                );
            } else {
                let decisions: Vec<(u64, ArqLevel, ArqLevel)> = outcome
                    .arq
                    .iter()
                    .map(|d| (d.slot, d.h_or_l, d.p))
                    .collect();
                assert_eq!(
                    decisions,
                    vec![
                        (164, ArqLevel::Minus, ArqLevel::Off),
                        (165, ArqLevel::Minus, ArqLevel::Off),
                        (166, ArqLevel::Minus, ArqLevel::Off),
                        (168, ArqLevel::Off, ArqLevel::Minus),
                        (169, ArqLevel::Off, ArqLevel::Minus),
                        (170, ArqLevel::Off, ArqLevel::Minus),
                    ],
                    "L-ARQ NAK is at packet start +44 and P-ARQ NAK at +48"
                );
                assert_eq!(
                    outcome.terminal_packets,
                    vec![TerminalPacketOutcome {
                        disposition: TerminalPacketDisposition::Exhausted,
                        payload_bits: 1024,
                        first_subpacket_start_slot: 120,
                        interlace: 0,
                        subpackets_accumulated: 4,
                        decoded_subpacket: None,
                    }]
                );
            }
        }
    }

    #[test]
    fn interlaces_track_concurrent_packets() {
        let f1024 = Subtype2DataFormat::for_payload_bits(1024).expect("format");
        let f256 = Subtype2DataFormat::for_payload_bits(256).expect("format");
        let p0 = build_packet_bits(1024, 0x1111_0003);
        let p1 = build_packet_bits(256, 0x2222_0004);

        let mut harq = SubframeHarq::new();
        let (w24a, w12a) = tx_subframe_symbols(f1024, &p0, 0, 0);
        let out0 = harq.ingest_subframe(120, 0, &detection(1024, 0), &w24a, &w12a);
        // Interlace 1 (slot 124) carries an unrelated 256-bit packet.
        let (w24b, w12b) = tx_subframe_symbols(f256, &p1, 0, 1);
        let out1 = harq.ingest_subframe(124, 0, &detection(256, 0), &w24b, &w12b);

        assert_eq!(
            out0.delivered.as_deref(),
            Some(&p0[..p0.len() - PACKET_FCS_BITS - PACKET_TAIL_BITS])
        );
        assert_eq!(
            out1.delivered.as_deref(),
            Some(&p1[..p1.len() - PACKET_FCS_BITS - PACKET_TAIL_BITS])
        );
    }

    #[test]
    fn unexpected_subpacket_timing_restarts_interlace_state() {
        let format = Subtype2DataFormat::for_payload_bits(1024).expect("format");
        let packet0 = build_packet_bits(1024, 0x3333_0007);
        let (w24, w12) = tx_subframe_symbols(format, &packet0, 0, 0);
        let corrupted: Vec<Complex32> = w24.iter().map(|c| -c * 0.01).collect();

        let mut harq = SubframeHarq::new();
        let first = harq.ingest_subframe(120, 0, &detection(1024, 0), &corrupted, &w12);
        assert!(!first.decoded);

        let packet1 = build_packet_bits(1024, 0x3333_0008);
        let (late_w24, late_w12) = tx_subframe_symbols(format, &packet1, 1, 0);
        let late = harq.ingest_subframe(252, 0, &detection(1024, 1), &late_w24, &late_w12);
        assert_eq!(late.subpacket_id, 1);
        assert_eq!(
            late.terminal_packets,
            vec![
                TerminalPacketOutcome {
                    disposition: TerminalPacketDisposition::Abandoned,
                    payload_bits: 1024,
                    first_subpacket_start_slot: 120,
                    interlace: 0,
                    subpackets_accumulated: 1,
                    decoded_subpacket: None,
                },
                TerminalPacketOutcome {
                    disposition: TerminalPacketDisposition::Decoded {
                        transmission_mode: HrpdTransmissionMode::HighCapacity,
                    },
                    payload_bits: 1024,
                    first_subpacket_start_slot: 252,
                    interlace: 0,
                    subpackets_accumulated: 1,
                    decoded_subpacket: Some(2),
                },
            ]
        );

        let state = harq.interlaces[0]
            .as_ref()
            .expect("unexpected subpacket becomes a fresh partial packet");
        assert_eq!(state.last_subpacket_start_slot, 252);
        assert_eq!(state.last_subpacket_id, 1);
        assert_eq!(state.subpackets_accumulated, 1);
    }

    #[test]
    fn null_rri_abandons_in_flight_packet() {
        let format = Subtype2DataFormat::for_payload_bits(1024).expect("format");
        let packet = build_packet_bits(1024, 0x3333_0007);
        let (w24, w12) = tx_subframe_symbols(format, &packet, 0, 0);
        let corrupted: Vec<Complex32> = w24.iter().map(|c| -c).collect();

        let mut harq = SubframeHarq::new();
        let first = harq.ingest_subframe(120, 0, &detection(1024, 0), &corrupted, &w12);
        assert!(!first.decoded);
        let null = harq.ingest_subframe(132, 0, &detection(0, 0), &[], &[]);
        assert!(!null.decoded && null.arq.is_empty());
        assert_eq!(
            null.terminal_packets,
            vec![TerminalPacketOutcome {
                disposition: TerminalPacketDisposition::Abandoned,
                payload_bits: 1024,
                first_subpacket_start_slot: 120,
                interlace: 0,
                subpackets_accumulated: 1,
                decoded_subpacket: None,
            }]
        );
        // A fresh packet on the same interlace decodes from scratch.
        let (w24b, w12b) = tx_subframe_symbols(format, &packet, 0, 0);
        let fresh = harq.ingest_subframe(144, 0, &detection(1024, 0), &w24b, &w12b);
        assert!(fresh.decoded);
        assert_eq!(
            fresh.delivered.as_deref(),
            Some(&packet[..packet.len() - PACKET_FCS_BITS - PACKET_TAIL_BITS])
        );
    }

    #[test]
    fn invalid_null_subpacket_does_not_clear_in_flight_packet() {
        let format = Subtype2DataFormat::for_payload_bits(1024).expect("format");
        let packet = build_packet_bits(1024, 0x4444_0008);
        let (w24, w12) = tx_subframe_symbols(format, &packet, 0, 0);
        let corrupted: Vec<Complex32> = w24.iter().map(|c| -c).collect();

        let mut harq = SubframeHarq::new();
        let first = harq.ingest_subframe(120, 0, &detection(1024, 0), &corrupted, &w12);
        assert!(!first.decoded);
        assert!(harq.interlaces[0].is_some(), "packet state established");

        let invalid = harq.ingest_subframe(132, 0, &detection(0, 2), &[], &[]);
        assert!(!invalid.decoded && invalid.arq.is_empty());
        assert!(
            harq.interlaces[0].is_some(),
            "payload index 0 with subpacket 2 is invalid, not null"
        );

        let null = harq.ingest_subframe(144, 0, &detection(0, 0), &[], &[]);
        assert!(!null.decoded && null.arq.is_empty());
        assert!(
            harq.interlaces[0].is_none(),
            "only payload index 0 with subpacket 0 clears the interlace"
        );
    }
}
