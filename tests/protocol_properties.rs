//! Property tests: the protocol parser must accept any input without panicking.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "property tests may fail loudly"
)]

use fluke_connect_client::{Reading, ReadingNotification};
use proptest::prelude::*;

proptest! {
    #[test]
    fn any_eight_bytes_decode_and_format(raw in any::<[u8; 8]>()) {
        let reading = Reading::from_array(raw);
        prop_assert_eq!(reading.raw(), &raw);
        // Every accessor and the Display impl must be total.
        let _ = reading.to_string();
        let _ = reading.value();
        let _ = reading.display_value();
        prop_assert!(reading.mantissa().unsigned_abs() <= 0x1F_FFFF);
        prop_assert!(reading.decimal_places() <= 7);
        prop_assert!(reading.range() <= 0x7F);
        // Enum codes round-trip to the bits they came from.
        prop_assert_eq!(u8::from(reading.unit()), raw[4]);
        prop_assert_eq!(u8::from(reading.function()), raw[5]);
    }

    #[test]
    fn any_sixteen_bytes_decode(raw in any::<[u8; 16]>()) {
        let n = ReadingNotification::from_bytes(&raw).unwrap();
        prop_assert_eq!(n.primary().raw(), &raw[..8]);
        let secondary_is_zero = raw[8..].iter().all(|&b| b == 0);
        prop_assert_eq!(n.secondary().is_none(), secondary_is_zero);
    }

    #[test]
    fn other_lengths_are_rejected(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
        prop_assume!(bytes.len() != 8 && bytes.len() != 16);
        prop_assert!(ReadingNotification::from_bytes(&bytes).is_err());
    }

    #[test]
    fn displayed_value_matches_mantissa_and_decimals(
        mantissa in 0_u32..0x1F_FFFF,
        negative: bool,
        decimals in 0_u8..=7,
    ) {
        let low = mantissa | (u32::from(decimals) << 25) | (u32::from(negative) << 31);
        let mut raw = [0_u8; 8];
        raw[..4].copy_from_slice(&low.to_le_bytes());
        let reading = Reading::from_array(raw);
        let expected = f64::from(mantissa) / 10_f64.powi(i32::from(decimals));
        let expected = if negative { -expected } else { expected };
        let actual = reading.display_value().unwrap();
        prop_assert!((actual - expected).abs() <= expected.abs() * 1e-12);
    }
}
