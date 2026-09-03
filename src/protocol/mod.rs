//! Wire-format parsing for the Fluke Connect Bluetooth Low Energy protocol.
//!
//! This module performs no I/O and only depends on `core`, so it can be unit
//! tested with byte fixtures and used from `no_std` targets.

pub mod enums;
pub mod error;
pub mod notification;
pub mod reading;
pub mod uuids;

pub use enums::{Attribute, Decade, Function, Magnitude, ReadingState, Unit};
pub use error::ProtocolError;
pub use notification::ReadingNotification;
pub use reading::Reading;

/// Decodes a hex string into bytes, panicking on malformed input so that a
/// typo in a fixture fails the test instead of silently decoding as zeros.
#[cfg(test)]
#[allow(
    clippy::panic,
    unused_qualifications,
    reason = "test helper; `Vec` is not in the prelude when the crate is built as no_std"
)]
pub(crate) fn test_hex(hex: &str) -> std::vec::Vec<u8> {
    assert!(hex.len() & 1 == 0, "odd-length hex fixture: {hex}");
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            let pair = hex.get(i..i.saturating_add(2)).unwrap_or_default();
            u8::from_str_radix(pair, 16)
                .unwrap_or_else(|_| panic!("bad hex pair {pair:?} in {hex}"))
        })
        .collect()
}
