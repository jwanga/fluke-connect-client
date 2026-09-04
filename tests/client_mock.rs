//! Drives [`FlukeDevice`] through a scripted in-memory transport.

#![cfg(feature = "std")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::unused_async_trait_impl,
    reason = "integration tests may fail loudly; the mock implements async trait methods synchronously"
)]

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fluke_connect_client::protocol::uuids;
use fluke_connect_client::transport::{BoxStream, Notification, Transport, TransportError};
use fluke_connect_client::{Error, FlukeDevice, Function, ProtocolError, ReadingState, Unit};
use futures_util::StreamExt as _;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

/// One recorded write: characteristic, value, with-response flag.
type Write = (u128, Vec<u8>, bool);

/// Scripted transport that records writes and replays notifications.
#[derive(Debug, Clone)]
struct MockTransport {
    /// Values returned by `read`, keyed by characteristic.
    values: Arc<Mutex<HashMap<u128, Vec<u8>>>>,
    /// Every write that was performed, in order.
    writes: Arc<Mutex<Vec<Write>>>,
    /// Characteristics that were subscribed to.
    subscriptions: Arc<Mutex<Vec<u128>>>,
    /// Channel used to inject notifications.
    tx: broadcast::Sender<Notification>,
}

impl MockTransport {
    fn new() -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            values: Arc::default(),
            writes: Arc::default(),
            subscriptions: Arc::default(),
            tx,
        }
    }

    fn with_value(self, characteristic: u128, value: &[u8]) -> Self {
        self.values
            .lock()
            .unwrap()
            .insert(characteristic, value.to_vec());
        self
    }

    fn notify(&self, characteristic: u128, value: &[u8]) {
        self.tx
            .send(Notification {
                characteristic,
                value: value.to_vec(),
            })
            .unwrap();
    }
}

impl Transport for MockTransport {
    async fn read(&self, characteristic: u128) -> Result<Vec<u8>, TransportError> {
        self.values
            .lock()
            .unwrap()
            .get(&characteristic)
            .cloned()
            .ok_or(TransportError::CharacteristicNotFound(characteristic))
    }

    async fn write(
        &self,
        characteristic: u128,
        value: &[u8],
        with_response: bool,
    ) -> Result<(), TransportError> {
        self.writes
            .lock()
            .unwrap()
            .push((characteristic, value.to_vec(), with_response));
        Ok(())
    }

    async fn subscribe(&self, characteristic: u128) -> Result<(), TransportError> {
        self.subscriptions.lock().unwrap().push(characteristic);
        Ok(())
    }

    async fn notifications(&self) -> Result<BoxStream<'static, Notification>, TransportError> {
        let rx = self.tx.subscribe();
        Ok(BroadcastStream::new(rx)
            .filter_map(|r| async move { r.ok() })
            .boxed())
    }

    async fn disconnect(&self) -> Result<(), TransportError> {
        Ok(())
    }
}

use common::hex;

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
