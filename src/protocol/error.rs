//! Errors produced while decoding protocol payloads.

/// A payload could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ProtocolError {
    /// The payload had an unexpected length.
    #[error("invalid payload length: expected {expected} bytes, got {actual}")]
    InvalidLength {
        /// The length the decoder expected.
        expected: usize,
        /// The length that was received.
        actual: usize,
    },
}
