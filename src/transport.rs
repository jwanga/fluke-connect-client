//! Abstraction over a connected GATT peripheral.
//!
//! Implement [`Transport`] to drive the [`FlukeDevice`](crate::FlukeDevice)
//! client with a Bluetooth stack other than the built-in btleplug backend,
//! or with a scripted test double.

use core::future::Future;

pub use futures_util::stream::BoxStream;

/// A value notification from a characteristic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    /// UUID of the characteristic that changed.
    pub characteristic: u128,
    /// The new value.
    pub value: Vec<u8>,
}

/// Errors a transport can report.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TransportError {
    /// The peripheral does not expose the requested characteristic.
    #[error("characteristic {0:032x} not found on device")]
    CharacteristicNotFound(u128),
    /// The connection is no longer open.
    #[error("not connected")]
    NotConnected,
    /// The operating system refused Bluetooth access.
    ///
    /// On macOS a command-line program inherits the Bluetooth permission of
    /// the terminal it runs in; grant it under *System Settings › Privacy &
    /// Security › Bluetooth*.
    #[error("Bluetooth permission denied by the operating system")]
    PermissionDenied,
    /// No Bluetooth adapter is available or it is powered off.
    #[error("no usable Bluetooth adapter")]
    NoAdapter,
    /// The operation did not complete in time.
    #[error("operation timed out")]
    Timeout,
    /// Any other backend failure.
    #[error("bluetooth backend error: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync + 'static>),
}

/// A connected GATT peripheral.
///
/// Characteristics are addressed by their 128-bit UUID as a `u128` (see
/// [`protocol::uuids`](crate::protocol::uuids)).
pub trait Transport: Send + Sync {
    /// Reads the current value of a characteristic.
    fn read(
        &self,
        characteristic: u128,
    ) -> impl Future<Output = Result<Vec<u8>, TransportError>> + Send;

    /// Writes a value to a characteristic.
    ///
    /// `with_response` selects a confirmed write; transports may ignore it
    /// when the characteristic supports only one write type.
    fn write(
        &self,
        characteristic: u128,
        value: &[u8],
        with_response: bool,
    ) -> impl Future<Output = Result<(), TransportError>> + Send;

    /// Enables notifications on a characteristic.
    fn subscribe(
        &self,
        characteristic: u128,
    ) -> impl Future<Output = Result<(), TransportError>> + Send;

    /// A stream of notifications from every subscribed characteristic.
    ///
    /// The stream must end when the connection is lost.
    fn notifications(
        &self,
    ) -> impl Future<Output = Result<BoxStream<'static, Notification>, TransportError>> + Send;

    /// Closes the connection.
    fn disconnect(&self) -> impl Future<Output = Result<(), TransportError>> + Send;
}
