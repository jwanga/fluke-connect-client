//! GATT service and characteristic UUIDs used by Fluke Connect devices.
//!
//! Fluke's vendor-specific attributes share the base UUID
//! `B698xxxx-7562-11E2-B50D-00163E46F8FE`, where `xxxx` follows the
//! Bluetooth SIG convention of `18xx` for services and `29xx` for
//! characteristics.
//!
//! UUIDs are expressed as `u128` so this module stays dependency free; the
//! `ble` feature converts them to `uuid::Uuid` where needed.

/// Builds a full 128-bit UUID from the 16-bit slot in Fluke's base UUID.
#[must_use]
pub const fn fluke_uuid(short: u16) -> u128 {
    let [hi, lo] = short.to_be_bytes();
    u128::from_be_bytes([
        0xB6, 0x98, hi, lo, 0x75, 0x62, 0x11, 0xE2, 0xB5, 0x0D, 0x00, 0x16, 0x3E, 0x46, 0xF8, 0xFE,
    ])
}

/// Builds a full 128-bit UUID from a Bluetooth SIG 16-bit UUID.
#[must_use]
pub const fn sig_uuid(short: u16) -> u128 {
    let [hi, lo] = short.to_be_bytes();
    u128::from_be_bytes([
        0x00, 0x00, hi, lo, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0x80, 0x5F, 0x9B, 0x34, 0xFB,
    ])
}

/// *Reading* service. Advertised by every Fluke Connect device; use it as
/// the scan filter.
pub const READING_SERVICE: u128 = fluke_uuid(0x1800);
/// 16-byte ASCII display string (read, notify). Not populated by the ir3000 FC.
pub const ASCII_READING: u128 = fluke_uuid(0x2901);
/// Binary reading record, see [`ReadingNotification`](super::ReadingNotification) (read, notify).
pub const BINARY_READING: u128 = fluke_uuid(0x290F);

/// *Connection* service: identity and housekeeping.
pub const CONNECTION_SERVICE: u128 = fluke_uuid(0x1801);
/// Module ID number shown on devices with a display, `u8` (read, write).
pub const ID_NUMBER: u128 = fluke_uuid(0x2902);
/// User-assignable device name, UTF-8 up to 98 bytes (read, write).
pub const USER_STRING: u128 = fluke_uuid(0x2903);
/// Writing any byte asks the device to drop the connection (write).
pub const FORCE_DROP: u128 = fluke_uuid(0x2904);
/// Locator LED: write `1` to blink, `0` to stop (write).
pub const LOCATOR: u128 = fluke_uuid(0x2905);
/// POSIX time as a little-endian `u64` (write, notify).
pub const POSIX_TIME: u128 = fluke_uuid(0x290E);
/// Host firmware update control point (read, write, notify). Not used by this crate.
pub const FIRMWARE_CONTROL: u128 = fluke_uuid(0x2911);
/// Host firmware update data buffer (write). Not used by this crate.
pub const FIRMWARE_BUFFER: u128 = fluke_uuid(0x2912);

/// Radio over-the-air download service. Not used by this crate.
pub const OAD_SERVICE: u128 = fluke_uuid(0x1804);
/// OAD image identify (write, notify). Not used by this crate.
pub const OAD_IMAGE_IDENTIFY: u128 = fluke_uuid(0x2913);
/// OAD image block (write, notify). Not used by this crate.
pub const OAD_IMAGE_BLOCK: u128 = fluke_uuid(0x2914);

/// Undocumented service present on the ir3000 FC. Not used by this crate.
pub const UNDOCUMENTED_SERVICE_1805: u128 = fluke_uuid(0x1805);

/// Bluetooth SIG *Device Information* service.
pub const DEVICE_INFORMATION_SERVICE: u128 = sig_uuid(0x180A);
/// Model number string, for example `FLUKE 289` (the attached meter on an ir3000 FC).
pub const MODEL_NUMBER: u128 = sig_uuid(0x2A24);
/// Serial number string of the attached meter.
pub const SERIAL_NUMBER: u128 = sig_uuid(0x2A25);
/// Firmware revision string.
pub const FIRMWARE_REVISION: u128 = sig_uuid(0x2A26);
/// Software revision string.
pub const SOFTWARE_REVISION: u128 = sig_uuid(0x2A28);
/// Manufacturer name string, for example `Fluke Mfg Co.`.
pub const MANUFACTURER_NAME: u128 = sig_uuid(0x2A29);

/// Bluetooth SIG *Battery* service.
pub const BATTERY_SERVICE: u128 = sig_uuid(0x180F);
/// Battery level in percent, `u8` (read, notify).
pub const BATTERY_LEVEL: u128 = sig_uuid(0x2A19);

#[cfg(test)]
mod tests {
    use super::{BATTERY_LEVEL, BINARY_READING, READING_SERVICE, fluke_uuid, sig_uuid};

    #[test]
    fn fluke_uuids_match_captured_values() {
        assert_eq!(READING_SERVICE, 0xB698_1800_7562_11E2_B50D_0016_3E46_F8FE);
        assert_eq!(BINARY_READING, 0xB698_290F_7562_11E2_B50D_0016_3E46_F8FE);
        assert_eq!(
            fluke_uuid(0x2905),
            0xB698_2905_7562_11E2_B50D_0016_3E46_F8FE
        );
    }

    #[test]
    fn sig_uuids_expand_correctly() {
        assert_eq!(BATTERY_LEVEL, 0x0000_2A19_0000_1000_8000_0080_5F9B_34FB);
        assert_eq!(sig_uuid(0x180A), 0x0000_180A_0000_1000_8000_0080_5F9B_34FB);
    }
}
