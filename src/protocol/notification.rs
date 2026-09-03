//! The reading notification sent on the *Binary Reading* characteristic.
//!
//! Devices in the Fluke Connect family send either one 8-byte
//! [`Reading`] or two back to back: the primary display followed by the
//! secondary display. An all-zero secondary record means the secondary
//! display is not in use.

use super::error::ProtocolError;
use super::reading::{READING_LEN, Reading};

/// Length of a notification carrying only a primary reading.
pub const SINGLE_LEN: usize = READING_LEN;

/// Length of a notification carrying primary and secondary readings.
pub const DUAL_LEN: usize = 2 * READING_LEN;

/// One notification from the *Binary Reading* characteristic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReadingNotification {
    /// The meter's primary display.
    primary: Reading,
    /// The meter's secondary display, when in use.
    secondary: Option<Reading>,
}

impl ReadingNotification {
    /// Parses an 8- or 16-byte notification payload.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::InvalidLength`] for any other length.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        match bytes.len() {
            SINGLE_LEN => Ok(Self {
                primary: Reading::from_bytes(bytes)?,
                secondary: None,
            }),
            DUAL_LEN => {
                let (first, second) = bytes.split_at(READING_LEN);
                let secondary = Reading::from_bytes(second)?;
                Ok(Self {
                    primary: Reading::from_bytes(first)?,
                    secondary: (!secondary.is_empty()).then_some(secondary),
                })
            }
            actual => Err(ProtocolError::InvalidLength {
                expected: DUAL_LEN,
                actual,
            }),
        }
    }

    /// The primary display reading.
    #[must_use]
    pub const fn primary(&self) -> &Reading {
        &self.primary
    }

    /// The secondary display reading, if the meter is showing one.
    #[must_use]
    pub const fn secondary(&self) -> Option<&Reading> {
        self.secondary.as_ref()
    }
}

#[cfg(test)]
#[allow(clippy::panic, reason = "tests may panic on unexpected input")]
mod tests {
    use std::vec::Vec;

    use super::ReadingNotification;
    use crate::protocol::enums::{Function, Unit};

    /// Decodes a hex string into bytes.
    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| {
                u8::from_str_radix(s.get(i..i.saturating_add(2)).unwrap_or("00"), 16).unwrap_or(0)
            })
            .collect()
    }

    #[test]
    fn primary_only_when_secondary_is_zero() {
        let n = ReadingNotification::from_bytes(&hex("01030002082200000000000000000000"))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(n.primary().display_value(), Some(76.9));
        assert!(n.secondary().is_none());
    }

    #[test]
    fn secondary_populated_in_loz_mode() {
        // Fluke 289 in V AC LoZ shows V DC on the secondary display.
        let n = ReadingNotification::from_bytes(&hex("00000002010700000000000202070000"))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(n.primary().function(), Function::VoltsAcLowZ);
        assert_eq!(n.primary().unit(), Unit::VoltsAc);
        let secondary = n.secondary().unwrap_or_else(|| panic!("secondary missing"));
        assert_eq!(secondary.unit(), Unit::VoltsDc);
        assert_eq!(secondary.function(), Function::VoltsAcLowZ);
    }

    #[test]
    fn eight_byte_payload_is_accepted() {
        let n = ReadingNotification::from_bytes(&hex("54150042020C0601"))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(n.primary().display_value(), Some(546.0));
        assert!(n.secondary().is_none());
    }

    #[test]
    fn other_lengths_are_rejected() {
        assert!(ReadingNotification::from_bytes(&[0; 12]).is_err());
        assert!(ReadingNotification::from_bytes(&[]).is_err());
    }
}
