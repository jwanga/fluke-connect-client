//! A reading from either characteristic, reduced to what both can answer.
//!
//! [`Measurement`] wraps a binary [`Reading`] or an [`AsciiReading`] and
//! exposes the accessors the two have in common. Source-specific detail
//! (function, attribute, range, hazardous-voltage flag, raw tokens) stays
//! reachable through [`Measurement::as_binary`] and [`Measurement::as_ascii`].
//!
//! The ASCII display's classification is folded onto [`ReadingState`]:
//!
//! | [`AsciiState`] | [`ReadingState`] |
//! |----------------|------------------|
//! | `Normal`       | `Normal`         |
//! | `OverRange`    | `OverRange`      |
//! | `Blank`        | `Blank`          |
//! | `Dashes`       | `Blank`          |
//! | `Other`        | `Invalid`        |
//!
//! An all-zero binary record (an unused display slot) reports
//! [`ReadingState::Empty`] and no value, even though [`Reading`] itself
//! decodes it as `0`.

use core::fmt;

use super::ascii::{AsciiReading, AsciiState};
use super::enums::{Magnitude, ReadingState, Unit};
use super::reading::{Reading, to_base_unit};

/// One measurement from a Fluke Connect device, whichever characteristic
/// it arrived on.
///
/// Construct with [`From`]. Accessors delegate to the wrapped type, so a
/// binary record and the ASCII text of the same display agree on value,
/// unit, magnitude, state and [`Display`](fmt::Display); the two places
/// where this type adds its own semantics are described in the
/// [module docs](self).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Measurement {
    /// Decoded from the 8-byte binary reading record.
    Binary(Reading),
    /// Decoded from the 16-character ASCII display string.
    Ascii(AsciiReading),
}

impl Measurement {
    /// The wrapped binary record, if this measurement came from one.
    #[must_use]
    pub const fn as_binary(&self) -> Option<&Reading> {
        match self {
            Self::Binary(reading) => Some(reading),
            Self::Ascii(_) => None,
        }
    }

    /// The wrapped ASCII display value, if this measurement came from one.
    #[must_use]
    pub const fn as_ascii(&self) -> Option<&AsciiReading> {
        match self {
            Self::Binary(_) => None,
            Self::Ascii(display) => Some(display),
        }
    }

    /// Display state, with ASCII classifications mapped onto
    /// [`ReadingState`] as described in the module docs and an all-zero
    /// binary record reported as [`ReadingState::Empty`].
    #[must_use]
    pub const fn state(&self) -> ReadingState {
        match self {
            Self::Binary(reading) => {
                if is_empty_record(reading) {
                    ReadingState::Empty
                } else {
                    reading.state()
                }
            }
            Self::Ascii(display) => state_of(display.state()),
        }
    }

    /// Unit of measure ([`Unit::None`] when the source could not name one).
    #[must_use]
    pub const fn unit(&self) -> Unit {
        match self {
            Self::Binary(reading) => reading.unit(),
            Self::Ascii(display) => display.unit(),
        }
    }

    /// SI prefix of the displayed value.
    #[must_use]
    pub const fn magnitude(&self) -> Magnitude {
        match self {
            Self::Binary(reading) => reading.magnitude(),
            Self::Ascii(display) => display.magnitude(),
        }
    }

    /// `true` when the device is showing a finite number: the wrapped
    /// reading's own `has_value`, except that an all-zero binary record is
    /// an empty slot and has no value.
    #[must_use]
    pub fn has_value(&self) -> bool {
        match self {
            Self::Binary(reading) => !reading.is_empty() && reading.has_value(),
            Self::Ascii(display) => display.has_value(),
        }
    }

    /// The number as shown on the display, in `magnitude`-prefixed units.
    ///
    /// `None` unless [`has_value`](Self::has_value) is true.
    #[must_use]
    pub fn display_value(&self) -> Option<f64> {
        if !self.has_value() {
            return None;
        }
        match self {
            Self::Binary(reading) => reading.display_value(),
            Self::Ascii(display) => display.display_value(),
        }
    }

    /// The value converted to the unit's SI base.
    ///
    /// `None` unless [`has_value`](Self::has_value) is true.
    #[must_use]
    pub fn value(&self) -> Option<f64> {
        self.display_value()
            .map(|v| to_base_unit(v, self.magnitude(), self.unit()))
    }
}

impl From<Reading> for Measurement {
    fn from(reading: Reading) -> Self {
        Self::Binary(reading)
    }
}

impl From<AsciiReading> for Measurement {
    fn from(display: AsciiReading) -> Self {
        Self::Ascii(display)
    }
}

/// Formats exactly as the wrapped reading does, so an all-zero binary
/// record prints `0`; callers that care should gate on
/// [`has_value`](Measurement::has_value).
impl fmt::Display for Measurement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Binary(reading) => fmt::Display::fmt(reading, f),
            Self::Ascii(display) => fmt::Display::fmt(display, f),
        }
    }
}

/// Maps an ASCII display classification onto the binary record's state
/// vocabulary (see the module docs).
const fn state_of(ascii: AsciiState) -> ReadingState {
    match ascii {
        AsciiState::Normal => ReadingState::Normal,
        AsciiState::OverRange => ReadingState::OverRange,
        AsciiState::Blank | AsciiState::Dashes => ReadingState::Blank,
        AsciiState::Other => ReadingState::Invalid,
    }
}

/// `true` for the all-zero record that marks an unused display slot.
///
/// A `const` twin of [`Reading::is_empty`] so that [`Measurement::state`]
/// can stay `const`.
const fn is_empty_record(reading: &Reading) -> bool {
    u64::from_le_bytes(*reading.raw()) == 0
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "tests may panic on unexpected input and compare exact decoded values"
)]
mod tests {
    use std::string::ToString as _;

    use super::Measurement;
    use crate::protocol::ascii::AsciiReading;
    use crate::protocol::enums::{ReadingState, Unit};
    use crate::protocol::reading::Reading;

    /// Wraps a binary record given as hex.
    fn binary(hex: &str) -> Measurement {
        Measurement::from(Reading::from_bytes(&crate::protocol::test_hex(hex)).unwrap())
    }

    /// Wraps an ASCII text.
    fn ascii(text: &[u8]) -> Measurement {
        Measurement::from(AsciiReading::from_bytes(text).unwrap())
    }

    #[test]
    fn binary_and_ascii_of_the_same_display_agree() {
        let b = binary("5c000002020c0000");
        let a = ascii(b"   9.2 V\x00  dc   ");
        assert_eq!(b.state(), a.state());
        assert_eq!(b.unit(), a.unit());
        assert_eq!(b.magnitude(), a.magnitude());
        assert_eq!(b.has_value(), a.has_value());
        assert_eq!(b.display_value(), a.display_value());
        assert_eq!(b.value(), a.value());
        assert_eq!(b.to_string(), a.to_string());
        assert_eq!(a.to_string(), "9.2 V DC");
    }

    #[test]
    fn ascii_states_map_onto_reading_states() {
        let cases: [(&[u8], ReadingState); 5] = [
            (b"   9.2 V\x00  dc   ", ReadingState::Normal),
            (b"    OL V\x00  ac   ", ReadingState::OverRange),
            (b"       V\x00  dc   ", ReadingState::Blank),
            (b"  ---- V\x00  dc   ", ReadingState::Blank),
            (b"  diSC F\x00       ", ReadingState::Invalid),
        ];
        for (text, expected) in cases {
            let m = ascii(text);
            assert_eq!(m.state(), expected, "{text:?}");
            assert_eq!(m.has_value(), expected == ReadingState::Normal, "{text:?}");
        }
    }

    #[test]
    fn all_zero_record_is_an_empty_slot() {
        let m = binary("0000000000000000");
        assert_eq!(m.state(), ReadingState::Empty);
        assert!(!m.has_value());
        assert_eq!(m.display_value(), None);
        assert_eq!(m.value(), None);
        assert_eq!(m.unit(), Unit::None);
        let inner = m.as_binary().unwrap();
        assert!(inner.is_empty());
        assert_eq!(inner.display_value(), Some(0.0));
    }

    #[test]
    fn escape_hatches() {
        let r = Reading::from_bytes(&crate::protocol::test_hex("5c000002020c0000")).unwrap();
        let m = Measurement::from(r);
        assert_eq!(m.as_binary(), Some(&r));
        assert!(m.as_ascii().is_none());
        let a = AsciiReading::from_bytes(b"   9.2 V\x00  dc   ").unwrap();
        let wrapped = Measurement::from(a);
        assert_eq!(wrapped.as_ascii(), Some(&a));
        assert!(wrapped.as_binary().is_none());
    }

    #[test]
    fn display_delegates_for_unrecognised_units() {
        assert_eq!(ascii(b"  12.0 PSI dc   ").to_string(), "12.0 PSI dc");
    }
}
