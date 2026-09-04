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
    /// An ASCII display value used a format byte this crate does not know.
    ///
    /// Format `0x00` is the documented "meter style" text layout. The
    /// ir3000 FC leaves the characteristic at a placeholder with format
    /// `0x01`, which the developer guide marks as unassigned.
    #[error("unsupported ASCII display format {0:#04x}")]
    UnsupportedFormat(u8),
    /// An ASCII display value contained a byte outside 7-bit ASCII.
    #[error("non-ASCII byte at offset {offset} in ASCII display value")]
    NotAscii {
        /// Zero-based offset of the offending byte within the 16-byte text.
        offset: usize,
    },
}
