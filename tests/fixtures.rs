//! Every record captured from real hardware must keep decoding.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests fail loudly"
)]

mod common;

use fluke_connect_client::{
    AsciiReading, AsciiState, Attribute, Decade, Function, Magnitude, Reading, ReadingNotification,
    ReadingState, Unit,
};

/// Distinct notifications captured from an ir3000 FC attached to a Fluke 289.
const IR3000FC_FLUKE289: &str = include_str!("fixtures/ir3000fc_fluke289_readings.txt");

/// ASCII display values published by owners of other Fluke Connect meters.
const ASCII_PUBLIC: &str = include_str!("fixtures/ascii_display_public.txt");

/// The ASCII fixture file's data lines as `(hex, expected display)` pairs.
fn ascii_records() -> impl Iterator<Item = (&'static str, &'static str)> {
    ASCII_PUBLIC
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let mut fields = l.split('\t');
            let hex = fields.next().unwrap_or_default();
            let expected = fields.next().unwrap_or_default();
            (hex, expected)
        })
}

#[test]
fn every_public_ascii_sample_decodes() {
    let mut count = 0;
    for (hex, expected) in ascii_records() {
        let r =
            AsciiReading::from_bytes(&common::hex(hex)).unwrap_or_else(|e| panic!("{hex}: {e}"));
        assert_eq!(r.to_string(), expected, "{hex}");
        assert_eq!(r.state(), AsciiState::Normal, "{hex}");
        assert!(
            !matches!(r.unit(), Unit::None | Unit::Unknown(_)),
            "unit not recognised in {hex}"
        );
        assert!(!matches!(r.magnitude(), Magnitude::Unknown(_)), "{hex}");
        assert!(r.value().is_some_and(f64::is_finite), "{hex}");
        count += 1;
    }
    assert!(
        count >= 3,
        "expected at least three public samples, found {count}"
    );
}

/// The fixture file's data lines.
fn records() -> impl Iterator<Item = &'static str> {
    IR3000FC_FLUKE289
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
}

/// A captured record must use only codes this crate knows, and its value
/// must be finite whenever it has one.
fn check_known(reading: &Reading, line: &str) {
    assert!(
        !matches!(reading.state(), ReadingState::Unknown(_)),
        "unknown state in {line}"
    );
    assert!(
        !matches!(reading.unit(), Unit::Unknown(_)),
        "unknown unit in {line}"
    );
    assert!(
        !matches!(reading.function(), Function::Unknown(_)),
        "unknown function in {line}"
    );
    assert!(
        !matches!(reading.magnitude(), Magnitude::Unknown(_)),
        "unknown magnitude in {line}"
    );
    assert!(
        !matches!(reading.decade(), Decade::Unknown(_)),
        "unknown decade in {line}"
    );
    assert!(
        !matches!(reading.attribute(), Attribute::Unknown(_)),
        "unknown attribute in {line}"
    );
    assert!(
        reading.value().is_none_or(f64::is_finite),
        "non-finite value in {line}"
    );
}

#[test]
fn every_captured_record_decodes() {
    let mut count = 0;
    for line in records() {
        let bytes = common::hex(line);
        let notification = ReadingNotification::from_bytes(&bytes)
            .unwrap_or_else(|e| panic!("record {line} failed to decode: {e}"));
        // Formatting must be total too.
        let _ = notification.primary().to_string();
        check_known(notification.primary(), line);
        if let Some(secondary) = notification.secondary() {
            check_known(secondary, line);
        }
        count += 1;
    }
    assert!(
        count > 100,
        "expected the full capture, decoded only {count} records"
    );
}

#[test]
fn capture_covers_the_interesting_states() {
    let states: Vec<ReadingState> = records()
        .map(|l| {
            ReadingNotification::from_bytes(&common::hex(l))
                .unwrap()
                .primary()
                .state()
        })
        .collect();
    for expected in [
        ReadingState::Normal,
        ReadingState::Invalid,
        ReadingState::Blank,
        ReadingState::OverRange,
    ] {
        assert!(
            states.contains(&expected),
            "capture lacks state {expected:?}"
        );
    }
    assert!(
        records().any(|l| ReadingNotification::from_bytes(&common::hex(l))
            .unwrap()
            .secondary()
            .is_some()),
        "capture lacks a populated secondary display"
    );
    assert!(
        records().any(|l| {
            ReadingNotification::from_bytes(&common::hex(l))
                .unwrap()
                .primary()
                .attribute()
                == Attribute::ShortCircuit
        }),
        "capture lacks the continuity short-circuit attribute"
    );
}
