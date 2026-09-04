//! Property tests: the protocol parser must accept any input without panicking.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "property tests may fail loudly"
)]

use fluke_connect_client::{AsciiReading, AsciiState, ProtocolError, Reading, ReadingNotification};
use proptest::prelude::*;

proptest! {
    #[test]
    fn any_eight_bytes_decode_and_format(raw in any::<[u8; 8]>()) {
        let reading = Reading::from_array(raw);
        prop_assert_eq!(reading.raw(), &raw);
        // Every accessor and the Display impl must be total.
        let _ = reading.to_string();
        prop_assert!(reading.value().is_none_or(f64::is_finite));
        prop_assert!(reading.display_value().is_none_or(f64::is_finite));
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

    #[test]
    fn any_ascii_text_decodes(raw in any::<[u8; 16]>().prop_map(|a| a.map(|b| b & 0x7F))) {
        let r = AsciiReading::from_array(raw).unwrap();
        prop_assert_eq!(r.raw(), &raw);
        prop_assert_eq!(r.text().len(), 16);
        let _ = r.to_string();
        prop_assert!(r.value().is_none_or(f64::is_finite));
        prop_assert!(r.display_value().is_none_or(f64::is_finite));
        prop_assert_eq!(r.has_value(), r.state() == AsciiState::Normal);
        prop_assert_eq!(r.has_value(), r.display_value().is_some());
        // The framed form decodes identically.
        let mut framed = [0_u8; 17];
        framed[1..].copy_from_slice(&raw);
        prop_assert_eq!(AsciiReading::from_bytes(&framed).unwrap(), r);
    }

    #[test]
    fn non_ascii_bytes_are_reported(raw in any::<[u8; 16]>(), pos in 0_usize..16, high in 0x80_u8..=0xFF) {
        let mut raw = raw.map(|b| b & 0x7F);
        raw[pos] = high;
        match AsciiReading::from_array(raw) {
            Err(ProtocolError::NotAscii { offset }) => prop_assert!(offset <= pos),
            other => prop_assert!(false, "expected NotAscii, got {other:?}"),
        }
    }

    #[test]
    fn non_zero_format_bytes_are_rejected(format in 1_u8..=u8::MAX, raw in any::<[u8; 16]>()) {
        let mut framed = [0_u8; 17];
        framed[0] = format;
        framed[1..].copy_from_slice(&raw);
        prop_assert_eq!(AsciiReading::from_bytes(&framed), Err(ProtocolError::UnsupportedFormat(format)));
    }

    #[test]
    fn other_ascii_lengths_are_rejected(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
        prop_assume!(bytes.len() != 16 && bytes.len() != 17);
        let rejected = matches!(
            AsciiReading::from_bytes(&bytes),
            Err(ProtocolError::InvalidLength { expected: 17, .. })
        );
        prop_assert!(rejected);
    }

    #[test]
    fn numeric_ascii_readings_round_trip(number in "-?[0-9]{1,4}(\\.[0-9]{1,2})?") {
        prop_assume!(number.len() <= 6);
        let text = format!("{number:>6} V\0  dc   ");
        let r = AsciiReading::from_bytes(text.as_bytes()).unwrap();
        prop_assert_eq!(r.state(), AsciiState::Normal);
        let expected: f64 = number.parse().unwrap();
        prop_assert!((r.display_value().unwrap() - expected).abs() <= 1e-9);
    }
}
