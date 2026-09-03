//! The crate-wide error type.

use crate::protocol::ProtocolError;
use crate::transport::TransportError;

/// Errors returned by the client and the built-in transport.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A payload from the device could not be decoded.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// The Bluetooth transport failed.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// The device sent a string that is not valid UTF-8.
    #[error("device string is not valid UTF-8")]
    InvalidUtf8,
    /// A name longer than the device allows was supplied.
    #[error("device name is {len} bytes; the maximum is {max}")]
    NameTooLong {
        /// Length of the supplied name in bytes.
        len: usize,
        /// Maximum length the device accepts.
        max: usize,
    },
    /// No matching device was found before the timeout elapsed.
    #[error("no Fluke Connect device found")]
    NotFound,
}

/// Convenience alias for results using [`Error`].
pub type Result<T, E = Error> = core::result::Result<T, E>;
