//! High-level client for a connected Fluke Connect device.

use futures_util::StreamExt as _;

use crate::error::{Error, Result};
use crate::protocol::{AsciiReading, ProtocolError, ReadingNotification, uuids};
use crate::transport::{BoxStream, Transport, TransportError};

/// Maximum length in bytes of the user-assignable device name.
pub const MAX_NAME_LEN: usize = 98;

/// Strings from the Bluetooth SIG *Device Information* service.
///
/// On an ir3000 FC adapter these describe the attached meter, not the
/// adapter itself. Any field the device does not expose is `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct DeviceInfo {
    /// Manufacturer name, for example `Fluke Mfg Co.`.
    pub manufacturer: Option<String>,
    /// Model number, for example `FLUKE 289`.
    pub model: Option<String>,
    /// Serial number.
    pub serial_number: Option<String>,
    /// Firmware revision.
    pub firmware_revision: Option<String>,
    /// Software revision.
    pub software_revision: Option<String>,
}

/// A connected Fluke Connect device.
///
/// Wraps any [`Transport`] and speaks the Fluke Connect GATT profile over
/// it: the binary reading stream, the ASCII display stream, and the
/// housekeeping characteristics. Obtain one from the built-in backend with `backend::Adapter::connect`
/// (feature `ble`), or construct it directly over your own transport with
/// [`FlukeDevice::new`].
#[derive(Debug)]
pub struct FlukeDevice<T> {
    /// The underlying GATT connection.
    transport: T,
}

impl<T: Transport> FlukeDevice<T> {
    /// Wraps an already connected transport.
    pub const fn new(transport: T) -> Self {
        Self { transport }
    }

    /// Borrows the underlying transport.
    pub const fn transport(&self) -> &T {
        &self.transport
    }

    /// Consumes the client and returns the transport.
    pub fn into_transport(self) -> T {
        self.transport
    }

    /// Subscribes to the binary reading characteristic and returns a stream
    /// of decoded readings.
    ///
    /// The stream ends when the transport reports that the connection was
    /// lost. Payloads that fail to decode are yielded as errors so a
    /// consumer can log them without losing the stream.
    ///
    /// # Errors
    ///
    /// Returns an error if the subscription cannot be established.
    pub async fn readings(&self) -> Result<BoxStream<'static, Result<ReadingNotification>>> {
        let values = self.subscribed(uuids::BINARY_READING).await?;
        Ok(values
            .map(|value| ReadingNotification::from_bytes(&value).map_err(Error::from))
            .boxed())
    }

    /// Reads the most recent reading without subscribing.
    ///
    /// # Errors
    ///
    /// Returns an error if the read fails or the payload cannot be decoded.
    pub async fn current_reading(&self) -> Result<ReadingNotification> {
        let bytes = self.transport.read(uuids::BINARY_READING).await?;
        Ok(ReadingNotification::from_bytes(&bytes)?)
    }

    /// Subscribes to the ASCII display characteristic and returns a stream
    /// of decoded display strings.
    ///
    /// The 376 FC and 902 FC clamps populate this characteristic and other
    /// family members may; the ir3000 FC exposes it but never notifies, so
    /// the stream stays silent there. Payloads that fail to decode are yielded as errors so a
    /// consumer can log them without losing the stream, which ends when the
    /// connection is lost.
    ///
    /// # Errors
    ///
    /// Returns an error if the subscription cannot be established.
    pub async fn ascii_readings(&self) -> Result<BoxStream<'static, Result<AsciiReading>>> {
        let values = self.subscribed(uuids::ASCII_READING).await?;
        Ok(values
            .map(|value| AsciiReading::from_bytes(&value).map_err(Error::from))
            .boxed())
    }

    /// Reads the current ASCII display value without subscribing.
    ///
    /// On an ir3000 FC this fails with [`ProtocolError::UnsupportedFormat`]
    /// because the adapter holds a placeholder value.
    ///
    /// # Errors
    ///
    /// Returns an error if the read fails or the payload cannot be decoded.
    pub async fn current_ascii_reading(&self) -> Result<AsciiReading> {
        let bytes = self.transport.read(uuids::ASCII_READING).await?;
        Ok(AsciiReading::from_bytes(&bytes)?)
    }

    /// Reads the *Device Information* service.
    ///
    /// Characteristics the device does not expose are left as `None`;
    /// any other transport failure is returned.
    ///
    /// # Errors
    ///
    /// Returns an error if a read fails for a reason other than the
    /// characteristic being absent.
    pub async fn device_info(&self) -> Result<DeviceInfo> {
        Ok(DeviceInfo {
            manufacturer: self.optional_string(uuids::MANUFACTURER_NAME).await?,
            model: self.optional_string(uuids::MODEL_NUMBER).await?,
            serial_number: self.optional_string(uuids::SERIAL_NUMBER).await?,
            firmware_revision: self.optional_string(uuids::FIRMWARE_REVISION).await?,
            software_revision: self.optional_string(uuids::SOFTWARE_REVISION).await?,
        })
    }

    /// Battery level in percent.
    ///
    /// # Errors
    ///
    /// Returns an error if the read fails or returns no data.
    pub async fn battery_level(&self) -> Result<u8> {
        first_byte(&self.transport.read(uuids::BATTERY_LEVEL).await?)
    }

    /// Subscribes to battery level changes.
    ///
    /// # Errors
    ///
    /// Returns an error if the subscription cannot be established.
    pub async fn battery_updates(&self) -> Result<BoxStream<'static, u8>> {
        let values = self.subscribed(uuids::BATTERY_LEVEL).await?;
        Ok(values
            .filter_map(|value| async move { value.first().copied() })
            .boxed())
    }

    /// The ID number shown on devices with a display (`0` when unset).
    ///
    /// On the ir3000 FC the value is per connection: it reads back after a
    /// write but resets to `0` when the connection drops.
    ///
    /// # Errors
    ///
    /// Returns an error if the read fails or returns no data.
    pub async fn id_number(&self) -> Result<u8> {
        first_byte(&self.transport.read(uuids::ID_NUMBER).await?)
    }

    /// Sets the ID number shown on devices with a display.
    ///
    /// # Errors
    ///
    /// Returns an error if the write fails.
    pub async fn set_id_number(&self, id: u8) -> Result<()> {
        Ok(self.transport.write(uuids::ID_NUMBER, &[id], true).await?)
    }

    /// The user-assignable device name.
    ///
    /// # Errors
    ///
    /// Returns an error if the read fails or the name is not UTF-8.
    pub async fn name(&self) -> Result<String> {
        let bytes = self.transport.read(uuids::USER_STRING).await?;
        decode_string(&bytes)
    }

    /// Sets the user-assignable device name (at most [`MAX_NAME_LEN`] bytes).
    ///
    /// # Errors
    ///
    /// Returns [`Error::NameTooLong`] if the name is too long, or an error
    /// if the write fails.
    pub async fn set_name(&self, name: &str) -> Result<()> {
        if name.len() > MAX_NAME_LEN {
            return Err(Error::NameTooLong {
                len: name.len(),
                max: MAX_NAME_LEN,
            });
        }
        Ok(self
            .transport
            .write(uuids::USER_STRING, name.as_bytes(), true)
            .await?)
    }

    /// Turns the locator LED on or off.
    ///
    /// # Errors
    ///
    /// Returns an error if the write fails.
    pub async fn set_locator(&self, on: bool) -> Result<()> {
        Ok(self
            .transport
            .write(uuids::LOCATOR, &[u8::from(on)], true)
            .await?)
    }

    /// Sets the device clock to a POSIX timestamp in seconds.
    ///
    /// The value is written as the 8-byte little-endian integer described
    /// in Fluke's developer guide. The ir3000 FC rejects every write to this
    /// characteristic with an attribute-length error, so expect a transport
    /// error there; whether other family members accept it is unverified.
    ///
    /// # Errors
    ///
    /// Returns an error if the write fails or the device rejects it.
    pub async fn set_time(&self, posix_seconds: u64) -> Result<()> {
        Ok(self
            .transport
            .write(uuids::POSIX_TIME, &posix_seconds.to_le_bytes(), true)
            .await?)
    }

    /// Asks the device to drop the connection from its side.
    ///
    /// The ir3000 FC accepts the write but was not observed to actually
    /// disconnect; prefer [`disconnect`](Self::disconnect).
    ///
    /// # Errors
    ///
    /// Returns an error if the write fails.
    pub async fn force_drop(&self) -> Result<()> {
        Ok(self.transport.write(uuids::FORCE_DROP, &[1], false).await?)
    }

    /// Closes the connection.
    ///
    /// # Errors
    ///
    /// Returns an error if the transport fails to disconnect.
    pub async fn disconnect(&self) -> Result<()> {
        Ok(self.transport.disconnect().await?)
    }

    /// Reads a string characteristic, mapping "not found" to `None`.
    async fn optional_string(&self, characteristic: u128) -> Result<Option<String>> {
        match self.transport.read(characteristic).await {
            Ok(bytes) => decode_string(&bytes).map(Some),
            Err(TransportError::CharacteristicNotFound(_)) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Opens the notification stream, enables notifications on one
    /// characteristic (in that order, so no early notification is lost) and
    /// returns only that characteristic's values.
    async fn subscribed(&self, characteristic: u128) -> Result<BoxStream<'static, Vec<u8>>> {
        let notifications = self.transport.notifications().await?;
        self.transport.subscribe(characteristic).await?;
        Ok(notifications
            .filter_map(
                move |n| async move { (n.characteristic == characteristic).then_some(n.value) },
            )
            .boxed())
    }
}

/// Extracts the single byte of a one-byte characteristic value.
fn first_byte(bytes: &[u8]) -> Result<u8> {
    bytes
        .first()
        .copied()
        .ok_or(Error::Protocol(ProtocolError::InvalidLength {
            expected: 1,
            actual: 0,
        }))
}

/// Decodes a device string, trimming the NUL padding and trailing spaces
/// that Fluke devices append.
fn decode_string(bytes: &[u8]) -> Result<String> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let text = bytes.get(..end).unwrap_or_default();
    core::str::from_utf8(text)
        .map(|s| s.trim_end().to_owned())
        .map_err(|_| Error::InvalidUtf8)
}

#[cfg(test)]
mod tests {
    use super::decode_string;

    #[test]
    fn strings_are_trimmed_of_nul_and_spaces() {
        assert_eq!(
            decode_string(b"01.00.01  \0").ok(),
            Some("01.00.01".to_owned())
        );
        assert_eq!(
            decode_string(b"FLUKE 289").ok(),
            Some("FLUKE 289".to_owned())
        );
        assert_eq!(decode_string(b"").ok(), Some(String::new()));
    }

    #[test]
    fn invalid_utf8_is_an_error() {
        assert!(decode_string(&[0xFF, 0xFE]).is_err());
    }
}
