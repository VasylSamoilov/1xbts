//! RFC 4733 §2.3 telephone events and §3.2 digit codes.

use std::collections::VecDeque;

const END_MASK: u8 = 0x80;
const RESERVED_MASK: u8 = 0x40;
const VOLUME_MASK: u8 = 0x3f;
const EVENT_OCTETS: usize = 4;
const RECENT_EVENTS: usize = 32;
const DIGITS: &[u8; 16] = b"0123456789*#ABCD";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Digit(u8);

impl Digit {
    #[cfg(test)]
    pub fn from_keypad(key: u8) -> Option<Self> {
        DIGITS
            .iter()
            .position(|digit| *digit == key)
            .map(|code| Self(code as u8))
    }

    pub fn keypad(self) -> u8 {
        DIGITS[usize::from(self.0)]
    }

    pub fn frequencies(self) -> [f32; 2] {
        const ROWS: [f32; 4] = [697.0, 770.0, 852.0, 941.0];
        const COLUMNS: [f32; 4] = [1209.0, 1336.0, 1477.0, 1633.0];
        const KEYPAD: &[u8; 16] = b"123A456B789C*0#D";
        let index = KEYPAD
            .iter()
            .position(|digit| *digit == self.keypad())
            .expect("digit is in keypad");
        [ROWS[index / 4], COLUMNS[index % 4]]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TelephoneEvent {
    pub digit: Digit,
    pub volume: u8,
    pub duration_samples: u16,
    pub ended: bool,
}

impl TelephoneEvent {
    #[cfg(test)]
    pub fn new(digit: Digit, duration_samples: u16, ended: bool) -> Self {
        const DEFAULT_VOLUME_DB: u8 = 10;
        Self {
            digit,
            volume: DEFAULT_VOLUME_DB,
            duration_samples,
            ended,
        }
    }

    pub fn parse(payload: &[u8]) -> Option<Self> {
        if payload.len() != EVENT_OCTETS
            || payload[0] as usize >= DIGITS.len()
            || payload[1] & RESERVED_MASK != 0
        {
            return None;
        }
        let duration_samples = u16::from_be_bytes([payload[2], payload[3]]);
        if duration_samples == 0 {
            return None;
        }
        Some(Self {
            digit: Digit(payload[0]),
            volume: payload[1] & VOLUME_MASK,
            duration_samples,
            ended: payload[1] & END_MASK != 0,
        })
    }

    #[cfg(test)]
    pub fn payload(self) -> [u8; EVENT_OCTETS] {
        let duration = self.duration_samples.to_be_bytes();
        [
            self.digit.0,
            self.volume.min(VOLUME_MASK) | if self.ended { END_MASK } else { 0 },
            duration[0],
            duration[1],
        ]
    }
}

#[derive(Default)]
pub(super) struct EventReceiver {
    recent: VecDeque<ReceivedEvent>,
}

struct ReceivedEvent {
    ssrc: u32,
    timestamp: u32,
    digit: Digit,
    duration_samples: u16,
    ended: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ToneUpdate {
    pub digit: Digit,
    pub start_sample: u16,
    pub duration_samples: u16,
    pub volume: u8,
}

impl EventReceiver {
    pub fn receive(
        &mut self,
        ssrc: u32,
        timestamp: u32,
        event: TelephoneEvent,
    ) -> Option<ToneUpdate> {
        if !self.recent.iter().any(|entry| {
            entry.ssrc == ssrc && entry.timestamp == timestamp && entry.digit == event.digit
        }) {
            if self.recent.len() == RECENT_EVENTS {
                self.recent.pop_front();
            }
            self.recent.push_back(ReceivedEvent {
                ssrc,
                timestamp,
                digit: event.digit,
                duration_samples: 0,
                ended: false,
            });
        }
        let entry = self
            .recent
            .iter_mut()
            .find(|entry| {
                entry.ssrc == ssrc && entry.timestamp == timestamp && entry.digit == event.digit
            })
            .expect("event inserted above");
        if entry.ended || event.duration_samples <= entry.duration_samples {
            return None;
        }
        let update = ToneUpdate {
            digit: event.digit,
            start_sample: entry.duration_samples,
            duration_samples: event.duration_samples - entry.duration_samples,
            volume: event.volume,
        };
        entry.duration_samples = event.duration_samples;
        entry.ended = event.ended;
        Some(update)
    }
}

impl ToneUpdate {
    pub fn pcm(&self) -> Vec<i16> {
        let [low_hz, high_hz] = self.digit.frequencies();
        let amplitude = f32::from(i16::MAX) * 0.5 * 10.0_f32.powf(-f32::from(self.volume) / 20.0);
        (u32::from(self.start_sample)
            ..u32::from(self.start_sample) + u32::from(self.duration_samples))
            .map(|sample| {
                let phase =
                    std::f32::consts::TAU * sample as f32 / crate::codec::PCM_SAMPLE_RATE_HZ as f32;
                (amplitude * ((phase * low_hz).sin() + (phase * high_hz).sin())).round() as i16
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_digits_round_trip_and_invalid_events_are_rejected() {
        for (code, key) in DIGITS.iter().enumerate() {
            let digit = Digit::from_keypad(*key).unwrap();
            let event = TelephoneEvent::new(digit, 800, true);
            assert_eq!(event.payload(), [code as u8, 0x8a, 3, 32]);
            assert_eq!(TelephoneEvent::parse(&event.payload()), Some(event));
            assert_eq!(digit.keypad(), *key);
        }
        assert!(Digit::from_keypad(b'X').is_none());
        for bad in [
            &[16, 0, 1, 0][..],
            &[0, 0x40, 1, 0],
            &[0, 0, 0, 0],
            &[0, 0, 1],
        ] {
            assert!(TelephoneEvent::parse(bad).is_none());
        }
    }

    #[test]
    fn retransmissions_and_reordering_do_not_duplicate_tones() {
        let mut receiver = EventReceiver::default();
        let digit = Digit::from_keypad(b'5').unwrap();
        let first = TelephoneEvent::new(digit, 160, false);
        let end = TelephoneEvent::new(digit, 800, true);
        assert_eq!(
            receiver.receive(1, 100, first).unwrap().duration_samples,
            160
        );
        let update = receiver.receive(1, 100, end).unwrap();
        assert_eq!((update.start_sample, update.duration_samples), (160, 640));
        for event in [end, first, end] {
            assert!(receiver.receive(1, 100, event).is_none());
        }
        assert!(receiver.receive(1, 900, end).is_some());
        assert!(receiver.receive(2, 100, end).is_some());
    }

    #[test]
    fn lost_start_packet_still_recovers_digit_and_pcm_is_continuous() {
        let digit = Digit::from_keypad(b'#').unwrap();
        let mut receiver = EventReceiver::default();
        let first = receiver
            .receive(1, 0, TelephoneEvent::new(digit, 160, false))
            .unwrap();
        let second = receiver
            .receive(1, 0, TelephoneEvent::new(digit, 800, true))
            .unwrap();
        let complete = EventReceiver::default()
            .receive(1, 0, TelephoneEvent::new(digit, 800, true))
            .unwrap();
        assert_eq!([first.pcm(), second.pcm()].concat(), complete.pcm());
        assert_eq!(complete.pcm().len(), 800);
    }
}
