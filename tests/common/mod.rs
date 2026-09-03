//! Helpers shared by the integration tests.

#![allow(
    dead_code,
    clippy::redundant_pub_crate,
    reason = "not every test binary uses every helper; pub(crate) satisfies unreachable_pub"
)]

/// Decodes a hex string, panicking on malformed input so a fixture typo
/// fails the test instead of decoding as zeros.
#[allow(clippy::panic, clippy::expect_used, reason = "test helper")]
pub(crate) fn hex(s: &str) -> Vec<u8> {
    assert!(s.len() & 1 == 0, "odd-length hex fixture: {s}");
    (0..s.len())
        .step_by(2)
        .map(|i| {
            let pair = s.get(i..i.saturating_add(2)).unwrap_or_default();
            u8::from_str_radix(pair, 16).unwrap_or_else(|_| panic!("bad hex pair {pair:?} in {s}"))
        })
        .collect()
}
