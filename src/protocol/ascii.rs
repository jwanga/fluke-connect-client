//! The text sent on the *ASCII Reading* characteristic.
//!
//! The GATT value is 17 bytes: a format byte (`0` selects the meter-style
//! layout below) followed by 16 ASCII characters. A bare 16-byte text is
//! also accepted in case a device or transport strips the format byte.
//!
//! | Offset | Len | Field      | Contents                                                     |
//! |--------|-----|------------|--------------------------------------------------------------|
//! | 0      | 6   | reading    | right-justified, space padded: digits, `.`, `-`, or `OL`     |
//! | 6      | 1   | multiplier | one of `n u m k M`, or a space                               |
//! | 7      | 4   | unit       | left-justified, NUL-terminated when short: `V`, `A`, `OHMS`, `DEGC`, `DEGF`, `H`, `VHZ`, `F`, `R` |
//! | 11     | 2   | acdc       | `ac`, `dc`, or spaces                                        |
//! | 13     | 1   | bolt       | `*` while the hazardous-voltage symbol is lit                |
//! | 14     | 2   | inrush     | `in` while inrush capture is active, else spaces             |
//!
//! Example: `00 20 20 20 39 2e 32 20 56 00 20 20 64 63 20 20 20` is the text
//! `"   9.2 V\0  dc   "`, that is **9.2 V DC** on a Fluke 376 FC.
//!
//! Public captures show the 376 FC and 902 FC clamps populating this
//! characteristic; other family members may as well. The ir3000 FC leaves
//! it at a placeholder whose format byte is `1`, which decodes to
//! [`ProtocolError::UnsupportedFormat`].

use core::fmt;
use core::ops::Range;

use super::enums::{Magnitude, Unit};
use super::error::ProtocolError;
use super::reading::to_base_unit;

/// Length of the ASCII text that follows the format byte.
pub const ASCII_TEXT_LEN: usize = 16;
/// Length of the full characteristic value: format byte plus text.
pub const ASCII_FRAMED_LEN: usize = 17;
/// Format byte announcing the meter-style layout parsed by this module.
pub const ASCII_FORMAT_METER: u8 = 0;

/// Byte range of the reading field within the text.
const READING_FIELD: Range<usize> = 0..6;
/// Byte range of the unit field within the text.
const UNIT_FIELD: Range<usize> = 7..11;
/// Byte range of the `ac`/`dc` field within the text.
const ACDC_FIELD: Range<usize> = 11..13;

/// How the reading field of an ASCII display should be interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum AsciiState {
    /// A finite decimal number is shown.
    Normal,
    /// `OL` or `-OL`: over range.
    OverRange,
    /// The reading field is empty.
    Blank,
    /// The reading field is all dashes, the "no value" display.
    Dashes,
    /// Any other text, for example `diSC` or `OPEn`; see
    /// [`AsciiReading::reading_text`].
    Other,
}

/// One decoded ASCII display value from a Fluke Connect device.
///
/// Construct with [`AsciiReading::from_bytes`]. Text tokens are exposed
/// verbatim (trimmed) so nothing is lost when a unit or state is not one
/// this crate recognises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct AsciiReading {
    /// Classification of the reading field.
    state: AsciiState,
    /// SI prefix decoded from the multiplier character.
    magnitude: Magnitude,
    /// Unit decoded from the unit token and the `ac`/`dc` field, or [`Unit::None`].
    unit: Unit,
    /// `true` while the hazardous-voltage (bolt) symbol is lit.
    hazardous_voltage: bool,
    /// `true` while inrush capture is active.
    inrush: bool,
    /// The 16 ASCII bytes this value was decoded from (format byte excluded).
    raw: [u8; ASCII_TEXT_LEN],
}

impl AsciiReading {
    /// Decodes a 17-byte characteristic value (format byte plus text) or a
    /// bare 16-byte text.
    ///
    /// # Examples
    ///
    /// ```
    /// use fluke_connect_client::{AsciiReading, AsciiState, Unit};
    ///
    /// // "   9.2 V dc" as sent by a Fluke 376 FC.
    /// let display = AsciiReading::from_bytes(b"\x00   9.2 V\x00  dc   ")?;
    /// assert_eq!(display.state(), AsciiState::Normal);
    /// assert_eq!(display.unit(), Unit::VoltsDc);
    /// assert_eq!(display.display_value(), Some(9.2));
    /// assert_eq!(display.to_string(), "9.2 V DC");
    /// # Ok::<(), fluke_connect_client::ProtocolError>(())
    /// ```
    ///
    /// # Errors
    ///
    /// - [`ProtocolError::InvalidLength`] for any length other than
    ///   [`ASCII_FRAMED_LEN`] or [`ASCII_TEXT_LEN`].
    /// - [`ProtocolError::UnsupportedFormat`] when the format byte is not
    ///   [`ASCII_FORMAT_METER`].
    /// - [`ProtocolError::NotAscii`] when the text contains a byte outside
    ///   7-bit ASCII.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if let Ok(&[format, text @ ..]) = <&[u8; ASCII_FRAMED_LEN]>::try_from(bytes) {
            if format != ASCII_FORMAT_METER {
                return Err(ProtocolError::UnsupportedFormat(format));
            }
            return Self::from_array(text);
        }
        let raw =
            <[u8; ASCII_TEXT_LEN]>::try_from(bytes).map_err(|_| ProtocolError::InvalidLength {
                expected: ASCII_FRAMED_LEN,
                actual: bytes.len(),
            })?;
        Self::from_array(raw)
    }

    /// Decodes the 16-byte text without a format byte.
    ///
    /// Every ASCII pattern decodes; a byte outside 7-bit ASCII is reported
    /// as [`ProtocolError::NotAscii`].
    fn from_array(raw: [u8; ASCII_TEXT_LEN]) -> Result<Self, ProtocolError> {
        if let Some(offset) = raw.iter().position(|b| !b.is_ascii()) {
            return Err(ProtocolError::NotAscii { offset });
        }
        let [_, _, _, _, _, _, multiplier, _, _, _, _, _, _, bolt, i0, i1] = raw;
        let text = trim_padding(field(&raw, READING_FIELD));
        let token = trim_padding(field(&raw, UNIT_FIELD));
        let coupling = trim_padding(field(&raw, ACDC_FIELD));
        Ok(Self {
            state: classify(text),
            magnitude: magnitude_of(multiplier),
            unit: unit_of(token, coupling),
            hazardous_voltage: bolt == b'*',
            inrush: [i0, i1] == *b"in",
            raw,
        })
    }

    /// The 16 raw text bytes.
    #[must_use]
    pub const fn raw(&self) -> &[u8; ASCII_TEXT_LEN] {
        &self.raw
    }

    /// The whole 16-character text verbatim, padding and NULs included.
    #[must_use]
    pub fn text(&self) -> &str {
        field(&self.raw, 0..ASCII_TEXT_LEN)
    }

    /// The reading field with padding removed, for example `9.2`, `-0.052`,
    /// `OL`, or an empty string for a blank display.
    #[must_use]
    pub fn reading_text(&self) -> &str {
        trim_padding(field(&self.raw, READING_FIELD))
    }

    /// The unit token with padding removed, for example `V`, `OHMS`, `DEGC`.
    #[must_use]
    pub fn unit_token(&self) -> &str {
        trim_padding(field(&self.raw, UNIT_FIELD))
    }

    /// The coupling token: `ac`, `dc`, or an empty string when not applicable.
    #[must_use]
    pub fn acdc(&self) -> &str {
        trim_padding(field(&self.raw, ACDC_FIELD))
    }

    /// Classification of the reading field.
    #[must_use]
    pub const fn state(&self) -> AsciiState {
        self.state
    }

    /// SI prefix of the displayed value.
    ///
    /// An unrecognised multiplier character is preserved as
    /// [`Magnitude::Unknown`] carrying its ASCII code.
    #[must_use]
    pub const fn magnitude(&self) -> Magnitude {
        self.magnitude
    }

    /// Unit of measure, or [`Unit::None`] when the unit token (combined with
    /// the `ac`/`dc` field) is not recognised; see [`unit_token`](Self::unit_token).
    #[must_use]
    pub const fn unit(&self) -> Unit {
        self.unit
    }

    /// `true` while the hazardous-voltage symbol is lit.
    #[must_use]
    pub const fn hazardous_voltage(&self) -> bool {
        self.hazardous_voltage
    }

    /// `true` while inrush capture is active.
    #[must_use]
    pub const fn inrush(&self) -> bool {
        self.inrush
    }

    /// `true` when the display shows a finite number.
    #[must_use]
    pub const fn has_value(&self) -> bool {
        matches!(self.state, AsciiState::Normal)
    }

    /// The number as shown, in `magnitude`-prefixed units (`9.2` for
    /// `9.2 V DC`, `0.0` for `0.0 µF`).
    ///
    /// Returns `None` unless [`has_value`](Self::has_value) is true.
    #[must_use]
    pub fn display_value(&self) -> Option<f64> {
        if !self.has_value() {
            return None;
        }
        self.reading_text().parse::<f64>().ok()
    }

    /// The value converted to the unit's SI base (`1500.0` for `1.50 kΩ`).
    ///
    /// Returns `None` unless [`has_value`](Self::has_value) is true.
    #[must_use]
    pub fn value(&self) -> Option<f64> {
        self.display_value()
            .map(|v| to_base_unit(v, self.magnitude, self.unit))
    }
}

impl fmt::Display for AsciiReading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = self.reading_text();
        let value = if text.is_empty() { "----" } else { text };
        if self.unit == Unit::None {
            let token = self.unit_token();
            let coupling = self.acdc();
            write!(f, "{value}")?;
            if !token.is_empty() {
                write!(f, " {}{token}", self.magnitude.symbol())?;
            }
            if !coupling.is_empty() {
                write!(f, " {coupling}")?;
            }
            Ok(())
        } else {
            write!(
                f,
                "{value} {}{}",
                self.magnitude.symbol(),
                self.unit.symbol()
            )
        }
    }
}

/// Slices a field out of the text as `&str`.
///
/// The ranges are constants inside the array and the text was validated as
/// ASCII, so both fallbacks are unreachable.
fn field(raw: &[u8; ASCII_TEXT_LEN], range: Range<usize>) -> &str {
    let bytes = raw.get(range).unwrap_or_default();
    core::str::from_utf8(bytes).unwrap_or_default()
}

/// Removes the space and NUL padding Fluke uses around tokens.
fn trim_padding(s: &str) -> &str {
    s.trim_matches(|c: char| c == ' ' || c == '\0')
}

/// Classifies the trimmed reading field.
fn classify(text: &str) -> AsciiState {
    if text.is_empty() {
        return AsciiState::Blank;
    }
    if text == "OL" || text == "-OL" {
        return AsciiState::OverRange;
    }
    if text.bytes().all(|b| b == b'-') {
        return AsciiState::Dashes;
    }
    // The character filter keeps `inf`, `nan` and exponents out of Normal.
    let numeric_chars = text
        .bytes()
        .all(|b| b.is_ascii_digit() || b == b'.' || b == b'-');
    if numeric_chars && text.parse::<f64>().is_ok_and(f64::is_finite) {
        return AsciiState::Normal;
    }
    AsciiState::Other
}

/// Maps the multiplier character to an SI prefix.
const fn magnitude_of(byte: u8) -> Magnitude {
    match byte {
        b'n' => Magnitude::Nano,
        b'u' => Magnitude::Micro,
        b'm' => Magnitude::Milli,
        b'k' => Magnitude::Kilo,
        b'M' => Magnitude::Mega,
        b' ' | 0 => Magnitude::None,
        other => Magnitude::Unknown(other),
    }
}

/// Maps the unit token and coupling to a [`Unit`].
///
/// `V` and `A` without an `ac`/`dc` qualifier deliberately map to
/// [`Unit::None`] rather than guessing; the token itself stays available.
/// The developer guide's `H` token is hertz, not henries.
fn unit_of(token: &str, coupling: &str) -> Unit {
    match (token, coupling) {
        ("V", "dc") => Unit::VoltsDc,
        ("V", "ac") => Unit::VoltsAc,
        ("A", "dc") => Unit::AmpsDc,
        ("A", "ac") => Unit::AmpsAc,
        ("OHMS", _) => Unit::Ohms,
        ("H", _) => Unit::Hertz,
        ("VHZ", _) => Unit::VoltsAcPerHertz,
        ("F", _) => Unit::Farads,
        ("DEGC", _) => Unit::Celsius,
        ("DEGF", _) => Unit::Fahrenheit,
        ("R", _) => Unit::Rankine,
        _ => Unit::None,
    }
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

    use super::{AsciiReading, AsciiState};
    use crate::protocol::enums::{Magnitude, Unit};
    use crate::protocol::error::ProtocolError;

    /// Decodes a byte string into a reading.
    fn ascii(bytes: &[u8]) -> AsciiReading {
        AsciiReading::from_bytes(bytes).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn fluke_376fc_volts_dc() {
        let r = ascii(&crate::protocol::test_hex(
            "00202020392e3220560020206463202020",
        ));
        assert_eq!(r.state(), AsciiState::Normal);
        assert_eq!(r.reading_text(), "9.2");
        assert_eq!(r.unit_token(), "V");
        assert_eq!(r.acdc(), "dc");
        assert_eq!(r.unit(), Unit::VoltsDc);
        assert_eq!(r.magnitude(), Magnitude::None);
        assert_eq!(r.display_value(), Some(9.2));
        assert_eq!(r.value(), Some(9.2));
        assert!(!r.hazardous_voltage());
        assert!(!r.inrush());
        assert_eq!(r.to_string(), "9.2 V DC");
    }

    #[test]
    fn ir3000_placeholder_is_unsupported_format() {
        let placeholder = crate::protocol::test_hex("0102030405000000000000000000000000");
        assert_eq!(
            AsciiReading::from_bytes(&placeholder),
            Err(ProtocolError::UnsupportedFormat(1))
        );
    }

    #[test]
    fn over_range_blank_dashes_and_other() {
        let ol = ascii(b"    OL V\x00  ac   ");
        assert_eq!(ol.state(), AsciiState::OverRange);
        assert_eq!(ol.unit(), Unit::VoltsAc);
        assert_eq!(ol.display_value(), None);
        assert_eq!(ol.to_string(), "OL V AC");

        let blank = ascii(b"                ");
        assert_eq!(blank.state(), AsciiState::Blank);
        assert_eq!(blank.to_string(), "----");

        let dashes = ascii(b"  ---- V\x00  dc   ");
        assert_eq!(dashes.state(), AsciiState::Dashes);
        assert_eq!(dashes.to_string(), "---- V DC");

        let other = ascii(b"  diSC F\x00       ");
        assert_eq!(other.state(), AsciiState::Other);
        assert_eq!(other.reading_text(), "diSC");
        assert_eq!(other.display_value(), None);

        let inf = ascii(b"   inf V\x00  dc   ");
        assert_eq!(inf.state(), AsciiState::Other);
    }

    #[test]
    fn kilo_ohms_scale_to_base_unit() {
        let r = ascii(b"  1.50kOHMS     ");
        assert_eq!(r.magnitude(), Magnitude::Kilo);
        assert_eq!(r.unit(), Unit::Ohms);
        assert_eq!(r.value(), Some(1500.0));
        assert_eq!(r.to_string(), "1.50 kΩ");
    }

    #[test]
    fn unknown_unit_token_keeps_the_text() {
        let r = ascii(b"  12.0 PSI dc   ");
        assert_eq!(r.unit(), Unit::None);
        assert_eq!(r.unit_token(), "PSI");
        assert_eq!(r.display_value(), Some(12.0));
        assert_eq!(r.to_string(), "12.0 PSI dc");
    }

    #[test]
    fn volts_without_coupling_are_not_guessed() {
        let r = ascii(b"   9.2 V\x00       ");
        assert_eq!(r.unit(), Unit::None);
        assert_eq!(r.to_string(), "9.2 V");
    }

    #[test]
    fn flags_and_unknown_multiplier() {
        let r = ascii(b" 230.5xV\x00  ac*in");
        assert!(r.hazardous_voltage());
        assert!(r.inrush());
        assert_eq!(r.magnitude(), Magnitude::Unknown(b'x'));
        assert_eq!(r.value(), Some(230.5));
    }

    #[test]
    fn remaining_unit_tokens_map() {
        assert_eq!(ascii(b"  25.0 DEGC     ").unit(), Unit::Celsius);
        assert_eq!(ascii(b"  77.0 DEGF     ").unit(), Unit::Fahrenheit);
        assert_eq!(ascii(b"  60.0 H\x00       ").unit(), Unit::Hertz);
        assert_eq!(ascii(b"   1.0 VHZ      ").unit(), Unit::VoltsAcPerHertz);
        assert_eq!(ascii(b" 500.0 R\x00       ").unit(), Unit::Rankine);
        assert_eq!(ascii(b"   1.0 A\x00  ac   ").unit(), Unit::AmpsAc);
    }
}
