//! The 8-byte binary reading record.
//!
//! Layout (two little-endian 32-bit words, bit 0 = least significant):
//!
//! | Word | Bits  | Field                                  |
//! |------|-------|----------------------------------------|
//! | 0    | 0-20  | absolute mantissa (21 bits)            |
//! | 0    | 21-24 | [`ReadingState`]                       |
//! | 0    | 25-27 | decimal places                         |
//! | 0    | 28-30 | [`Magnitude`]                          |
//! | 0    | 31    | sign (1 = negative)                    |
//! | 1    | 0-7   | [`Unit`]                               |
//! | 1    | 8-15  | [`Function`]                           |
//! | 1    | 16-22 | range number                           |
//! | 1    | 23-25 | [`Decade`]                             |
//! | 1    | 26-30 | [`Attribute`]                          |
//! | 1    | 31    | capture flag                           |
//!
//! The displayed value is `mantissa / 10^decimal_places` in
//! `magnitude`-prefixed `unit`s. A mantissa of `0x1F_FFFF` is a sentinel
//! meaning "no value" and is shown as dashes by the Fluke Connect app.

use core::fmt;

use super::enums::{Attribute, Decade, Function, Magnitude, ReadingState, Unit};
use super::error::ProtocolError;

/// Size in bytes of one reading record.
pub const READING_LEN: usize = 8;

/// Mantissa value the device sends when there is no number to show.
pub const NO_VALUE_MANTISSA: u32 = 0x1F_FFFF;

/// Mask selecting the 21-bit mantissa.
const MANTISSA_MASK: u32 = 0x1F_FFFF;

/// One decoded reading from a Fluke Connect device.
///
/// Construct with [`Reading::from_bytes`]. All fields are exposed through
/// accessors so that the wire representation can evolve without breaking
/// callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Reading {
    /// Signed mantissa (`-0x1F_FFFF..=0x1F_FFFF`).
    mantissa: i32,
    /// Number of digits after the decimal point in the displayed value.
    decimal_places: u8,
    /// SI prefix of the displayed value.
    magnitude: Magnitude,
    /// Display state.
    state: ReadingState,
    /// Unit of measure.
    unit: Unit,
    /// Meter function.
    function: Function,
    /// Range number (device specific).
    range: u8,
    /// Range decade hint.
    decade: Decade,
    /// Reading qualifier.
    attribute: Attribute,
    /// Capture flag (set when the reading was captured on the device).
    capture: bool,
    /// The raw bytes this reading was decoded from.
    raw: [u8; READING_LEN],
}

impl Reading {
    /// Decodes an 8-byte reading record.
    ///
    /// # Examples
    ///
    /// ```
    /// use fluke_connect_client::{Reading, ReadingState, Unit};
    ///
    /// // 546.0 mV DC, as captured from a pc3000 FC.
    /// let reading = Reading::from_bytes(&[0x54, 0x15, 0x00, 0x42, 0x02, 0x0C, 0x06, 0x01])?;
    /// assert_eq!(reading.state(), ReadingState::Normal);
    /// assert_eq!(reading.unit(), Unit::VoltsDc);
    /// assert_eq!(reading.display_value(), Some(546.0));
    /// assert_eq!(reading.value(), Some(0.546));
    /// # Ok::<(), fluke_connect_client::ProtocolError>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::InvalidLength`] when `bytes` is not exactly
    /// [`READING_LEN`] bytes long.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let raw: [u8; READING_LEN] =
            bytes.try_into().map_err(|_| ProtocolError::InvalidLength {
                expected: READING_LEN,
                actual: bytes.len(),
            })?;
        Ok(Self::from_array(raw))
    }

    /// Decodes an 8-byte reading record from a fixed-size array.
    ///
    /// This cannot fail: every bit pattern decodes to *some* reading,
    /// with unrecognised codes preserved in `Unknown` variants.
    #[must_use]
    pub fn from_array(raw: [u8; READING_LEN]) -> Self {
        let [l0, l1, l2, l3, h0, h1, h2, h3] = raw;
        let low = u32::from_le_bytes([l0, l1, l2, l3]);
        let high = u32::from_le_bytes([h0, h1, h2, h3]);

        let abs_mantissa = low & MANTISSA_MASK;
        let negative = (low >> 31) == 1;
        // The mantissa is at most 21 bits so the conversion cannot fail;
        // fall back to the sentinel rather than panicking if it ever did.
        let signed = i32::try_from(abs_mantissa).unwrap_or(i32::MAX);
        let mantissa = if negative {
            signed.wrapping_neg()
        } else {
            signed
        };

        Self {
            mantissa,
            decimal_places: low_byte((low >> 25) & 0x7),
            magnitude: Magnitude::from_raw(low_byte((low >> 28) & 0x7)),
            state: ReadingState::from_raw(low_byte((low >> 21) & 0xF)),
            unit: Unit::from_raw(low_byte(high & 0xFF)),
            function: Function::from_raw(low_byte((high >> 8) & 0xFF)),
            range: low_byte((high >> 16) & 0x7F),
            decade: Decade::from_raw(low_byte((high >> 23) & 0x7)),
            attribute: Attribute::from_raw(low_byte((high >> 26) & 0x1F)),
            capture: (high >> 31) == 1,
            raw,
        }
    }

    /// The raw 8 bytes this reading was decoded from.
    #[must_use]
    pub const fn raw(&self) -> &[u8; READING_LEN] {
        &self.raw
    }

    /// `true` when every byte of the record is zero, which is how the device
    /// marks an unused reading slot (for example an empty secondary display).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.raw == [0; READING_LEN]
    }

    /// Signed integer mantissa before the decimal point is applied.
    #[must_use]
    pub const fn mantissa(&self) -> i32 {
        self.mantissa
    }

    /// Number of digits after the decimal point in the displayed value.
    #[must_use]
    pub const fn decimal_places(&self) -> u8 {
        self.decimal_places
    }

    /// SI prefix of the displayed value.
    #[must_use]
    pub const fn magnitude(&self) -> Magnitude {
        self.magnitude
    }

    /// Display state.
    #[must_use]
    pub const fn state(&self) -> ReadingState {
        self.state
    }

    /// Unit of measure.
    #[must_use]
    pub const fn unit(&self) -> Unit {
        self.unit
    }

    /// Meter function.
    #[must_use]
    pub const fn function(&self) -> Function {
        self.function
    }

    /// Device-specific range number.
    #[must_use]
    pub const fn range(&self) -> u8 {
        self.range
    }

    /// Range decade hint.
    #[must_use]
    pub const fn decade(&self) -> Decade {
        self.decade
    }

    /// Reading qualifier.
    #[must_use]
    pub const fn attribute(&self) -> Attribute {
        self.attribute
    }

    /// Capture flag.
    #[must_use]
    pub const fn capture(&self) -> bool {
        self.capture
    }

    /// `true` when the device has a number to show, that is the state is
    /// [`ReadingState::Normal`] and the mantissa is not the "no value"
    /// sentinel.
    #[must_use]
    pub fn has_value(&self) -> bool {
        self.state == ReadingState::Normal && self.mantissa.unsigned_abs() != NO_VALUE_MANTISSA
    }

    /// The value as shown on the meter display, in `magnitude`-prefixed
    /// units (for example `546.0` for `546.0 mV DC`).
    ///
    /// Returns `None` when [`has_value`](Self::has_value) is false.
    #[must_use]
    pub fn display_value(&self) -> Option<f64> {
        self.has_value()
            .then(|| f64::from(self.mantissa) / pow10(self.decimal_places))
    }

    /// The value converted to the unit's SI base (for example `0.546` for
    /// `546.0 mV DC`, or `4.61e12` for `4.61 TΩ` as ohms).
    ///
    /// Returns `None` when [`has_value`](Self::has_value) is false.
    #[must_use]
    pub fn value(&self) -> Option<f64> {
        let exponent = self
            .magnitude
            .exponent()
            .saturating_add(self.unit.base_exponent());
        self.display_value().map(|v| scale(v, exponent))
    }
}

impl fmt::Display for Reading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let suffix = UnitSuffix(self);
        if let Some(value) = self.display_value() {
            let places = usize::from(self.decimal_places);
            return write!(f, "{value:.places$} {suffix}");
        }
        match self.state {
            ReadingState::Normal | ReadingState::Blank | ReadingState::Empty => {
                write!(f, "---- {suffix}")
            }
            ReadingState::OverRange | ReadingState::OverloadA2d => write!(f, "OL {suffix}"),
            ReadingState::Inactive
            | ReadingState::Invalid
            | ReadingState::OpenThermocouple
            | ReadingState::Discharge
            | ReadingState::Leads
            | ReadingState::GreaterThan
            | ReadingState::MissingPhase
            | ReadingState::Error
            | ReadingState::LessThan
            | ReadingState::Unknown(_) => write!(f, "{:?}", self.state),
        }
    }
}

/// Formats the SI prefix and unit symbol of a reading, for example `mV DC`.
struct UnitSuffix<'a>(&'a Reading);

impl fmt::Display for UnitSuffix<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.0.magnitude.symbol(), self.0.unit.symbol())
    }
}

/// Extracts the low byte of a value already masked to fit in 8 bits.
const fn low_byte(value: u32) -> u8 {
    let [b0, _, _, _] = value.to_le_bytes();
    b0
}

/// Multiplies `value` by `10^exp`, dividing for negative exponents so that
/// results such as `52 / 1000` stay as close to the decimal value as `f64`
/// allows.
fn scale(value: f64, exp: i8) -> f64 {
    if exp < 0 {
        value / pow10(exp.unsigned_abs())
    } else {
        value * pow10(exp.unsigned_abs())
    }
}

/// `10^exp` for the exponent range that can appear in a reading.
///
/// The reachable range is the sum of [`Magnitude::exponent`] (-12..=9) and
/// [`Unit::base_exponent`] (-6..=12), so the table covers 0..=22 with
/// margin. Implemented with a table so the protocol module needs no `std`
/// or `libm` support for `powi`.
fn pow10(exp: u8) -> f64 {
    const POSITIVE: [f64; 23] = [
        1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
        1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
    ];
    POSITIVE.get(usize::from(exp)).copied().unwrap_or(f64::NAN)
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

    use super::{NO_VALUE_MANTISSA, Reading, pow10, scale};
    use crate::protocol::enums::{Function, Magnitude, ReadingState, Unit};

    /// Decodes a hex string into a reading.
    fn reading(hex: &str) -> Reading {
        Reading::from_bytes(&crate::protocol::test_hex(hex)).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn temperature_from_ir3000_fc_and_fluke_289() {
        let r = reading("0103000208220000");
        assert_eq!(r.mantissa(), 769);
        assert_eq!(r.decimal_places(), 1);
        assert_eq!(r.magnitude(), Magnitude::None);
        assert_eq!(r.state(), ReadingState::Normal);
        assert_eq!(r.unit(), Unit::Fahrenheit);
        assert_eq!(r.function(), Function::Temperature);
        assert_eq!(r.display_value(), Some(76.9));
        assert_eq!(r.value(), Some(76.9));
        assert_eq!(r.to_string(), "76.9 °F");
    }

    #[test]
    fn millivolts_dc_from_pc3000_capture() {
        // Published pc3000 FC capture: 546.0 mV DC.
        let r = reading("54150042020C0601");
        assert_eq!(r.unit(), Unit::VoltsDc);
        assert_eq!(r.function(), Function::VoltsDc);
        assert_eq!(r.magnitude(), Magnitude::Milli);
        assert_eq!(r.display_value(), Some(546.0));
        assert!((r.value().unwrap_or(0.0) - 0.546).abs() < 1e-9);
        assert_eq!(r.range(), 6);
        assert_eq!(r.to_string(), "546.0 mV DC");
    }

    #[test]
    fn negative_volts_dc() {
        // Published pc3000 FC capture: -0.758 V DC.
        let r = reading("F6020086020C0600");
        assert_eq!(r.mantissa(), -758);
        assert_eq!(r.display_value(), Some(-0.758));
        assert_eq!(r.to_string(), "-0.758 V DC");
    }

    #[test]
    fn negative_millivolts_dc_from_fluke_289() {
        let r = reading("340000c6020b0000");
        assert_eq!(r.function(), Function::MilliVoltsDc);
        assert_eq!(r.magnitude(), Magnitude::Milli);
        assert_eq!(r.display_value(), Some(-0.052));
        assert!((r.value().unwrap_or(0.0) - -0.000_052).abs() < 1e-12);
    }

    #[test]
    fn invalid_sentinel_after_function_change() {
        let r = reading("ffff7f00000c0000");
        assert_eq!(r.state(), ReadingState::Invalid);
        assert_eq!(r.mantissa().unsigned_abs(), NO_VALUE_MANTISSA);
        assert!(!r.has_value());
        assert_eq!(r.display_value(), None);
        assert_eq!(r.function(), Function::VoltsDc);
    }

    #[test]
    fn over_range_capacitance() {
        let r = reading("ffff9f420f2d0000");
        assert_eq!(r.state(), ReadingState::OverRange);
        assert_eq!(r.function(), Function::Capacitance);
        assert_eq!(r.unit(), Unit::Farads);
        assert!(!r.has_value());
        assert_eq!(r.to_string(), "OL mF");
    }

    #[test]
    fn blank_while_autoranging_ohms() {
        let r = reading("ffff3f220b280000");
        assert_eq!(r.state(), ReadingState::Blank);
        assert_eq!(r.function(), Function::Resistance);
        assert_eq!(r.to_string(), "---- MΩ");
    }

    #[test]
    fn low_ohms() {
        let r = reading("881300060b2a0000");
        assert_eq!(r.function(), Function::LowOhms);
        assert_eq!(r.unit(), Unit::Ohms);
        assert_eq!(r.decimal_places(), 3);
        assert_eq!(r.display_value(), Some(5.0));
        assert_eq!(r.to_string(), "5.000 Ω");
    }

    #[test]
    fn empty_slot() {
        let r = reading("0000000000000000");
        assert!(r.is_empty());
        assert_eq!(r.state(), ReadingState::Normal);
        assert_eq!(r.display_value(), Some(0.0));
    }

    #[test]
    fn wrong_length_is_an_error() {
        assert!(Reading::from_bytes(&[0; 7]).is_err());
        assert!(Reading::from_bytes(&[0; 9]).is_err());
    }

    #[test]
    fn giga_tera_ohms_stay_finite() {
        // Magnitude Giga (1) with unit TeraOhms (44): the largest exponent.
        let r = reading("0100001c2c280000");
        assert_eq!(r.magnitude(), Magnitude::Giga);
        assert_eq!(r.unit(), Unit::TeraOhms);
        assert!(r.value().is_some_and(f64::is_finite));
    }

    #[test]
    fn tera_ohms_scale_to_base_unit() {
        // 4.61 TΩ: mantissa 461, 2 decimals, unit TeraOhms (44 = 0x2C).
        let r = reading("cd0100042c280000");
        assert_eq!(r.unit(), Unit::TeraOhms);
        assert!((r.value().unwrap_or(0.0) - 4.61e12).abs() < 1.0);
    }

    #[test]
    fn pow10_table() {
        assert!((pow10(0) - 1.0).abs() < f64::EPSILON);
        assert!((pow10(3) - 1000.0).abs() < f64::EPSILON);
        assert!((scale(52.0, -3) - 0.052).abs() < f64::EPSILON);
        assert!(pow10(100).is_nan());
    }
}
