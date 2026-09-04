//! Drives [`FlukeDevice`] through a scripted in-memory transport.

#![cfg(feature = "std")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "integration tests may fail loudly; the mock implements async trait methods synchronously"
)]

mod common;

use fluke_connect_client::protocol::uuids;
use fluke_connect_client::transport::TransportError;
use fluke_connect_client::{Error, FlukeDevice, Function, ProtocolError, ReadingState, Unit};

/// Binary record with a populated secondary display (289 in V AC `LoZ`).
const LOZ: &str = "00000002010700000000000202070000";
/// Binary temperature record.
const TEMPERATURE: &str = "01030002082200000000000000000000";
/// ASCII 9.2 V DC from a 376 FC.
const ASCII_VOLTS: &str = "00202020392e3220560020206463202020";
/// The ir3000 FC placeholder on the ASCII characteristic.
const PLACEHOLDER: &str = "0102030405000000000000000000000000";
use futures_util::StreamExt as _;

use common::hex;
use common::mock::MockTransport;

#[tokio::test]
async fn readings_stream_decodes_binary_notifications_only() {
    let mock = MockTransport::new();
    let device = FlukeDevice::new(mock.clone());
    let mut readings = device.readings().await.unwrap();
    assert_eq!(
        mock.subscriptions.lock().unwrap().as_slice(),
        &[uuids::BINARY_READING]
    );

    mock.notify(uuids::BATTERY_LEVEL, &[60]);
    mock.notify(
        uuids::BINARY_READING,
        &hex("01030002082200000000000000000000"),
    );
    mock.notify(
        uuids::BINARY_READING,
        &hex("00000002010700000000000202070000"),
    );
    mock.notify(uuids::BINARY_READING, &[1, 2, 3]);

    let first = readings.next().await.unwrap().unwrap();
    assert_eq!(first.primary().display_value(), Some(76.9));
    assert_eq!(first.primary().unit(), Unit::Fahrenheit);
    assert!(first.secondary().is_none());

    let second = readings.next().await.unwrap().unwrap();
    assert_eq!(second.primary().function(), Function::VoltsAcLowZ);
    assert_eq!(second.secondary().unwrap().unit(), Unit::VoltsDc);

    let bad = readings.next().await.unwrap();
    assert!(matches!(bad, Err(Error::Protocol(_))));
}

#[tokio::test]
async fn readings_stream_ends_when_transport_closes() {
    let mock = MockTransport::new();
    let device = FlukeDevice::new(mock.clone());
    let mut readings = device.readings().await.unwrap();
    drop(mock);
    drop(device);
    assert!(readings.next().await.is_none());
}

#[tokio::test]
async fn device_info_reads_strings_and_tolerates_missing_ones() {
    let mock = MockTransport::new()
        .with_value(uuids::MODEL_NUMBER, b"FLUKE 289")
        .with_value(uuids::FIRMWARE_REVISION, b"01.00.01  \0")
        .with_value(uuids::MANUFACTURER_NAME, b"Fluke Mfg Co.\0");
    let device = FlukeDevice::new(mock);
    let info = device.device_info().await.unwrap();
    assert_eq!(info.model.as_deref(), Some("FLUKE 289"));
    assert_eq!(info.firmware_revision.as_deref(), Some("01.00.01"));
    assert_eq!(info.manufacturer.as_deref(), Some("Fluke Mfg Co."));
    assert_eq!(info.serial_number, None);
    assert_eq!(info.software_revision, None);
}

#[tokio::test]
async fn housekeeping_writes_use_documented_encodings() {
    let mock = MockTransport::new()
        .with_value(uuids::BATTERY_LEVEL, &[0x3c])
        .with_value(uuids::ID_NUMBER, &[0])
        .with_value(uuids::USER_STRING, b"IR 3000 FC");
    let device = FlukeDevice::new(mock.clone());

    assert_eq!(device.battery_level().await.unwrap(), 60);
    assert_eq!(device.id_number().await.unwrap(), 0);
    assert_eq!(device.name().await.unwrap(), "IR 3000 FC");

    device.set_locator(true).await.unwrap();
    device.set_locator(false).await.unwrap();
    device.set_id_number(7).await.unwrap();
    device.set_name("bench").await.unwrap();
    device.set_time(0x0102_0304).await.unwrap();
    device.force_drop().await.unwrap();

    let writes = mock.writes.lock().unwrap();
    assert_eq!(writes[0], (uuids::LOCATOR, vec![1], true));
    assert_eq!(writes[1], (uuids::LOCATOR, vec![0], true));
    assert_eq!(writes[2], (uuids::ID_NUMBER, vec![7], true));
    assert_eq!(writes[3], (uuids::USER_STRING, b"bench".to_vec(), true));
    assert_eq!(
        writes[4],
        (uuids::POSIX_TIME, vec![4, 3, 2, 1, 0, 0, 0, 0], true)
    );
    assert_eq!(writes[5], (uuids::FORCE_DROP, vec![1], false));
}

#[tokio::test]
async fn overlong_names_are_rejected_before_writing() {
    let mock = MockTransport::new();
    let device = FlukeDevice::new(mock.clone());
    let long = "x".repeat(99);
    assert!(matches!(
        device.set_name(&long).await,
        Err(Error::NameTooLong { len: 99, max: 98 })
    ));
    assert!(mock.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn current_reading_reads_the_characteristic() {
    let mock = MockTransport::new().with_value(uuids::BINARY_READING, &hex("ffff9f420f2d0000"));
    let device = FlukeDevice::new(mock);
    let reading = device.current_reading().await.unwrap();
    assert_eq!(reading.primary().state(), ReadingState::OverRange);
    assert_eq!(reading.primary().function(), Function::Capacitance);
}

#[tokio::test]
async fn ascii_readings_stream_decodes_ascii_notifications_only() {
    let mock = MockTransport::new();
    let device = FlukeDevice::new(mock.clone());
    let mut readings = device.ascii_readings().await.unwrap();
    assert_eq!(
        mock.subscriptions.lock().unwrap().as_slice(),
        &[uuids::ASCII_READING]
    );

    mock.notify(
        uuids::BINARY_READING,
        &hex("01030002082200000000000000000000"),
    );
    mock.notify(
        uuids::ASCII_READING,
        &hex("00202020392e3220560020206463202020"),
    );
    mock.notify(
        uuids::ASCII_READING,
        &hex("0102030405000000000000000000000000"),
    );
    mock.notify(uuids::ASCII_READING, &[1, 2, 3]);

    let first = readings.next().await.unwrap().unwrap();
    assert_eq!(first.unit(), Unit::VoltsDc);
    assert_eq!(first.display_value(), Some(9.2));

    let placeholder = readings.next().await.unwrap();
    assert!(matches!(
        placeholder,
        Err(Error::Protocol(ProtocolError::UnsupportedFormat(1)))
    ));

    let short = readings.next().await.unwrap();
    assert!(matches!(
        short,
        Err(Error::Protocol(ProtocolError::InvalidLength { .. }))
    ));
}

#[tokio::test]
async fn current_ascii_reading_reads_the_characteristic() {
    let mock = MockTransport::new().with_value(uuids::ASCII_READING, b"\x00   0.0uF\x00       ");
    let device = FlukeDevice::new(mock);
    let reading = device.current_ascii_reading().await.unwrap();
    assert_eq!(reading.to_string(), "0.0 µF");
}

#[tokio::test]
async fn current_ascii_reading_reports_a_missing_characteristic() {
    let device = FlukeDevice::new(MockTransport::new());
    assert!(matches!(
        device.current_ascii_reading().await,
        Err(Error::Transport(TransportError::CharacteristicNotFound(_)))
    ));
}

#[tokio::test]
async fn measurements_prefer_binary_once_it_notifies() {
    let mock = MockTransport::new();
    let device = FlukeDevice::new(mock.clone());
    let mut measurements = device.measurements().await.unwrap();
    assert_eq!(
        mock.subscriptions.lock().unwrap().as_slice(),
        &[uuids::BINARY_READING, uuids::ASCII_READING]
    );

    mock.notify(uuids::ASCII_READING, &hex(ASCII_VOLTS));
    mock.notify(uuids::BATTERY_LEVEL, &[60]);
    mock.notify(uuids::BINARY_READING, &hex(LOZ));
    mock.notify(uuids::ASCII_READING, &hex(ASCII_VOLTS));
    mock.notify(uuids::BINARY_READING, &hex(TEMPERATURE));
    mock.notify(uuids::BINARY_READING, &[1, 2, 3]);

    let first = measurements.next().await.unwrap().unwrap();
    assert!(first.primary().as_ascii().is_some());
    assert_eq!(first.primary().display_value(), Some(9.2));
    assert!(first.secondary().is_none());

    let second = measurements.next().await.unwrap().unwrap();
    assert_eq!(
        second.primary().as_binary().unwrap().function(),
        Function::VoltsAcLowZ
    );
    assert_eq!(second.secondary().unwrap().unit(), Unit::VoltsDc);

    // The ASCII notification after lock-on is dropped: the next item is the
    // temperature record, not 9.2 V.
    let third = measurements.next().await.unwrap().unwrap();
    assert_eq!(third.primary().display_value(), Some(76.9));
    assert_eq!(third.primary().unit(), Unit::Fahrenheit);

    let fourth = measurements.next().await.unwrap();
    assert!(matches!(
        fourth,
        Err(Error::Protocol(ProtocolError::InvalidLength { .. }))
    ));
}

#[tokio::test]
async fn measurements_use_binary_when_ascii_is_missing() {
    let mock = MockTransport::new().without_characteristic(uuids::ASCII_READING);
    let device = FlukeDevice::new(mock.clone());
    let mut measurements = device.measurements().await.unwrap();
    assert_eq!(
        mock.subscriptions.lock().unwrap().as_slice(),
        &[uuids::BINARY_READING]
    );

    mock.notify(uuids::BINARY_READING, &hex(TEMPERATURE));
    let item = measurements.next().await.unwrap().unwrap();
    assert_eq!(item.primary().display_value(), Some(76.9));
    assert!(item.secondary().is_none());
}

#[tokio::test]
async fn measurements_use_ascii_when_binary_is_missing() {
    let mock = MockTransport::new().without_characteristic(uuids::BINARY_READING);
    let device = FlukeDevice::new(mock.clone());
    let mut measurements = device.measurements().await.unwrap();
    assert_eq!(
        mock.subscriptions.lock().unwrap().as_slice(),
        &[uuids::ASCII_READING]
    );

    mock.notify(uuids::ASCII_READING, &hex(ASCII_VOLTS));
    mock.notify(uuids::ASCII_READING, &hex(PLACEHOLDER));
    mock.notify(uuids::ASCII_READING, &hex(ASCII_VOLTS));

    let first = measurements.next().await.unwrap().unwrap();
    assert_eq!(first.primary().to_string(), "9.2 V DC");
    let second = measurements.next().await.unwrap();
    assert!(matches!(
        second,
        Err(Error::Protocol(ProtocolError::UnsupportedFormat(1)))
    ));
    // Never locks without a binary notification.
    let third = measurements.next().await.unwrap().unwrap();
    assert!(third.primary().as_ascii().is_some());
}

#[tokio::test]
async fn measurements_fail_when_neither_characteristic_exists() {
    let mock = MockTransport::new()
        .without_characteristic(uuids::BINARY_READING)
        .without_characteristic(uuids::ASCII_READING);
    let device = FlukeDevice::new(mock.clone());
    assert!(matches!(
        device.measurements().await,
        Err(Error::NoReadingCharacteristic)
    ));
    assert!(mock.subscriptions.lock().unwrap().is_empty());
}

#[tokio::test]
async fn measurements_propagate_other_subscribe_errors() {
    let device = FlukeDevice::new(MockTransport::new().failing_subscribe());
    assert!(matches!(
        device.measurements().await,
        Err(Error::Transport(TransportError::NotConnected))
    ));
}

#[tokio::test]
async fn measurements_stream_ends_when_transport_closes() {
    let mock = MockTransport::new();
    let device = FlukeDevice::new(mock.clone());
    let mut measurements = device.measurements().await.unwrap();
    mock.drop_link();
    assert!(measurements.next().await.is_none());
}
