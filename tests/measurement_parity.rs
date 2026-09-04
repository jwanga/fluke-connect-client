//! A binary record and the ASCII text of the same display must agree once
//! wrapped in `Measurement`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests fail loudly"
)]

mod common;

use fluke_connect_client::{AsciiReading, Measurement, Reading, ReadingState};

/// One display rendered by both characteristics.
struct Pair {
    /// The 8-byte binary record as hex.
    binary: &'static str,
    /// The 16-character ASCII text.
    ascii: &'static [u8; 16],
    /// Expected `Display` output from both sides.
    display: &'static str,
    /// Expected state from both sides.
    state: ReadingState,
}

/// Binary records come from captures or the documented layout; the ASCII
/// texts for the 376 FC / 902 FC rows are public captures, the rest are
/// synthesized from the developer guide's layout.
const PAIRS: [Pair; 11] = [
    Pair {
        binary: "54150042020c0000",
        ascii: b" 546.0mV\x00  dc   ",
        display: "546.0 mV DC",
        state: ReadingState::Normal,
    },
    Pair {
        binary: "f6020086020c0000",
        ascii: b"-0.758 V\x00  dc   ",
        display: "-0.758 V DC",
        state: ReadingState::Normal,
    },
    Pair {
        binary: "5c000002020c0000",
        ascii: b"   9.2 V\x00  dc   ",
        display: "9.2 V DC",
        state: ReadingState::Normal,
    },
    Pair {
        binary: "0700000204150000",
        ascii: b"   0.7 A\x00  dc   ",
        display: "0.7 A DC",
        state: ReadingState::Normal,
    },
    Pair {
        binary: "960000340b280000",
        ascii: b"  1.50kOHMS     ",
        display: "1.50 kΩ",
        state: ReadingState::Normal,
    },
    Pair {
        binary: "000000520f2d0000",
        ascii: b"   0.0uF\x00       ",
        display: "0.0 µF",
        state: ReadingState::Normal,
    },
    Pair {
        binary: "0103000208220000",
        ascii: b"  76.9 DEGF     ",
        display: "76.9 °F",
        state: ReadingState::Normal,
    },
    Pair {
        binary: "fa00000207220000",
        ascii: b"  25.0 DEGC     ",
        display: "25.0 °C",
        state: ReadingState::Normal,
    },
    Pair {
        binary: "5802000205170000",
        ascii: b"  60.0 H\x00       ",
        display: "60.0 Hz",
        state: ReadingState::Normal,
    },
    Pair {
        binary: "ffff9f0201020000",
        ascii: b"    OL V\x00  ac   ",
        display: "OL V AC",
        state: ReadingState::OverRange,
    },
    Pair {
        binary: "ffff3f02020c0000",
        ascii: b"       V\x00  dc   ",
        display: "---- V DC",
        state: ReadingState::Blank,
    },
];

#[test]
fn every_pair_agrees_through_measurement() {
    let mut count = 0;
    for pair in &PAIRS {
        let binary = Measurement::from(Reading::from_bytes(&common::hex(pair.binary)).unwrap());
        let ascii = Measurement::from(AsciiReading::from_bytes(pair.ascii).unwrap());
        assert_eq!(binary.state(), pair.state, "{}", pair.display);
        assert_eq!(ascii.state(), pair.state, "{}", pair.display);
        assert_eq!(binary.unit(), ascii.unit(), "{}", pair.display);
        assert_eq!(binary.magnitude(), ascii.magnitude(), "{}", pair.display);
        assert_eq!(binary.has_value(), ascii.has_value(), "{}", pair.display);
        assert_eq!(
            binary.display_value(),
            ascii.display_value(),
            "{}",
            pair.display
        );
        assert_eq!(binary.value(), ascii.value(), "{}", pair.display);
        assert_eq!(binary.to_string(), pair.display);
        assert_eq!(ascii.to_string(), pair.display);
        assert_eq!(binary.has_value(), pair.state == ReadingState::Normal);
        count += 1;
    }
    assert_eq!(count, 11);
}
