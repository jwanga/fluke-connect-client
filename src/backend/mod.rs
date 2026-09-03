//! Built-in Bluetooth transport backed by [btleplug](https://crates.io/crates/btleplug).
//!
//! [`Adapter`] discovers Fluke Connect devices and connects to them,
//! producing a [`FlukeDevice`] over a [`BtleplugTransport`]. No btleplug
//! types are exposed, so this backend can evolve independently of the
//! public API.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use ::btleplug::api::{
    Central as _, CentralEvent, CentralState, Characteristic, Manager as _, Peripheral as _,
    ScanFilter, WriteType,
};
use ::btleplug::platform::{self, Manager, PeripheralId};
use futures_util::StreamExt as _;
use uuid::Uuid;

use crate::client::FlukeDevice;
use crate::error::{Error, Result};
use crate::protocol::uuids::READING_SERVICE;
use crate::transport::{BoxStream, Notification, Transport, TransportError};

/// How long [`Adapter::connect`] waits for the GATT connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// A Fluke Connect device seen while scanning.
#[derive(Debug, Clone)]
pub struct DiscoveredDevice {
    /// Backend identifier used to connect.
    id: PeripheralId,
    /// Advertised local name, if any.
    name: Option<String>,
    /// Bluetooth address as reported by the platform.
    address: String,
    /// Signal strength in dBm at the time of discovery, if known.
    rssi: Option<i16>,
}

impl DiscoveredDevice {
    /// Advertised local name, for example `IR 3000 FC`.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Platform address string (a MAC address on Linux and Windows, an
    /// opaque UUID on macOS).
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Signal strength in dBm at discovery time.
    #[must_use]
    pub const fn rssi(&self) -> Option<i16> {
        self.rssi
    }
}

impl fmt::Display for DiscoveredDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} ({})",
            self.name.as_deref().unwrap_or("<unnamed>"),
            self.address
        )?;
        if let Some(rssi) = self.rssi {
            write!(f, " {rssi} dBm")?;
        }
        Ok(())
    }
}

/// A Bluetooth adapter used to find and connect to Fluke Connect devices.
#[derive(Debug, Clone)]
pub struct Adapter {
    /// The platform adapter.
    inner: platform::Adapter,
}

impl Adapter {
    /// Opens the system's first Bluetooth adapter.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::NoAdapter`] when there is none, and
    /// [`TransportError::PermissionDenied`] when the operating system
    /// refuses access.
    pub async fn open() -> Result<Self> {
        let manager = Manager::new().await.map_err(map_err)?;
        let inner = manager
            .adapters()
            .await
            .map_err(map_err)?
            .into_iter()
            .next()
            .ok_or(TransportError::NoAdapter)?;
        Ok(Self { inner })
    }

    /// Human-readable description of the adapter.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend cannot describe the adapter.
    pub async fn info(&self) -> Result<String> {
        Ok(self.inner.adapter_info().await.map_err(map_err)?)
    }

    /// Whether the adapter reports itself powered on.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend cannot report the state.
    pub async fn is_powered_on(&self) -> Result<bool> {
        let state = self.inner.adapter_state().await.map_err(map_err)?;
        Ok(matches!(state, CentralState::PoweredOn))
    }

    /// Scans for Fluke Connect devices for `timeout` and returns everything
    /// found.
    ///
    /// # Errors
    ///
    /// Returns an error if scanning cannot be started.
    pub async fn scan(&self, timeout: Duration) -> Result<Vec<DiscoveredDevice>> {
        self.scan_inner(timeout, false).await
    }

    /// Scans until the first Fluke Connect device appears or `timeout`
    /// elapses.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] if nothing was seen in time.
    pub async fn find_first(&self, timeout: Duration) -> Result<DiscoveredDevice> {
        self.scan_inner(timeout, true)
            .await?
            .into_iter()
            .next()
            .ok_or(Error::NotFound)
    }

    /// Scans for the first device and connects to it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] if no device appears within `timeout`,
    /// or a transport error if the connection fails.
    pub async fn connect_first(&self, timeout: Duration) -> Result<FlukeDevice<BtleplugTransport>> {
        let device = self.find_first(timeout).await?;
        self.connect(&device).await
    }

    /// Connects to a discovered device and discovers its GATT table.
    ///
    /// # Errors
    ///
    /// Returns a transport error if the connection or service discovery
    /// fails.
    pub async fn connect(
        &self,
        device: &DiscoveredDevice,
    ) -> Result<FlukeDevice<BtleplugTransport>> {
        let peripheral = self.inner.peripheral(&device.id).await.map_err(map_err)?;
        #[cfg(feature = "tracing")]
        tracing::debug!(device = %device, "connecting");
        if let Err(e) = peripheral.connect_with_timeout(CONNECT_TIMEOUT).await {
            // A timed-out connect can leave the attempt pending on the OS side.
            let _ = peripheral.disconnect().await;
            return Err(map_err(e).into());
        }
        if let Err(e) = peripheral.discover_services().await {
            let _ = peripheral.disconnect().await;
            return Err(map_err(e).into());
        }
        let characteristics = peripheral
            .characteristics()
            .into_iter()
            .map(|c| (c.uuid.as_u128(), c))
            .collect();
        Ok(FlukeDevice::new(BtleplugTransport {
            adapter: self.inner.clone(),
            peripheral,
            characteristics,
        }))
    }

    /// Shared scan loop. Filters on the Fluke reading service both in the
    /// platform filter and again on the advertisement, because `BlueZ` merges
    /// scan filters from all D-Bus clients.
    async fn scan_inner(
        &self,
        timeout: Duration,
        stop_on_first: bool,
    ) -> Result<Vec<DiscoveredDevice>> {
        let service = Uuid::from_u128(READING_SERVICE);
        let mut events = self.inner.events().await.map_err(map_err)?;
        self.inner
            .start_scan(ScanFilter {
                services: vec![service],
            })
            .await
            .map_err(map_err)?;

        let mut found: Vec<DiscoveredDevice> = Vec::new();
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                () = &mut deadline => break,
                event = events.next() => {
                    let Some(event) = event else { break };
                    let (CentralEvent::DeviceDiscovered(id)
                    | CentralEvent::DeviceUpdated(id)
                    | CentralEvent::ServicesAdvertisement { id, .. }) = event
                    else {
                        continue;
                    };
                    if found.iter().any(|d| d.id == id) {
                        continue;
                    }
                    if let Some(device) = self.describe(&id, service).await {
                        found.push(device);
                        if stop_on_first {
                            break;
                        }
                    }
                }
            }
        }
        // Stopping the scan is best effort; a failure here must not hide results.
        let _ = self.inner.stop_scan().await;
        Ok(found)
    }

    /// Builds a [`DiscoveredDevice`] if the peripheral advertises `service`.
    async fn describe(&self, id: &PeripheralId, service: Uuid) -> Option<DiscoveredDevice> {
        let peripheral = self.inner.peripheral(id).await.ok()?;
        let props = peripheral.properties().await.ok().flatten()?;
        if !props.services.contains(&service) {
            return None;
        }
        // CoreBluetooth hides MAC addresses and btleplug reports all zeros;
        // fall back to the platform's peripheral identifier there.
        let address = if props.address.into_inner() == [0; 6] {
            id.to_string()
        } else {
            props.address.to_string()
        };
        Some(DiscoveredDevice {
            id: id.clone(),
            name: props.local_name,
            address,
            rssi: props.rssi,
        })
    }
}

/// [`Transport`] implementation over a connected btleplug peripheral.
#[derive(Debug, Clone)]
pub struct BtleplugTransport {
    /// Adapter the peripheral belongs to; used to observe disconnects.
    adapter: platform::Adapter,
    /// The connected peripheral.
    peripheral: platform::Peripheral,
    /// Characteristics discovered on the peripheral, by 128-bit UUID.
    characteristics: HashMap<u128, Characteristic>,
}

impl BtleplugTransport {
    /// Looks up a characteristic by UUID.
    fn characteristic(&self, uuid: u128) -> Result<&Characteristic, TransportError> {
        self.characteristics
            .get(&uuid)
            .ok_or(TransportError::CharacteristicNotFound(uuid))
    }

    /// UUIDs of every characteristic the device exposes.
    pub fn characteristic_uuids(&self) -> impl Iterator<Item = u128> + '_ {
        self.characteristics.keys().copied()
    }
}

impl Transport for BtleplugTransport {
    async fn read(&self, characteristic: u128) -> Result<Vec<u8>, TransportError> {
        let c = self.characteristic(characteristic)?;
        self.peripheral.read(c).await.map_err(map_err)
    }

    async fn write(
        &self,
        characteristic: u128,
        value: &[u8],
        with_response: bool,
    ) -> Result<(), TransportError> {
        let c = self.characteristic(characteristic)?;
        let kind = if with_response {
            WriteType::WithResponse
        } else {
            WriteType::WithoutResponse
        };
        self.peripheral.write(c, value, kind).await.map_err(map_err)
    }

    async fn subscribe(&self, characteristic: u128) -> Result<(), TransportError> {
        let c = self.characteristic(characteristic)?;
        self.peripheral.subscribe(c).await.map_err(map_err)
    }

    async fn notifications(&self) -> Result<BoxStream<'static, Notification>, TransportError> {
        let id = self.peripheral.id();
        let values = self
            .peripheral
            .notifications()
            .await
            .map_err(map_err)?
            .map(|n| Notification {
                characteristic: n.uuid.as_u128(),
                value: n.value,
            });
        let disconnected = self
            .adapter
            .events()
            .await
            .map_err(map_err)?
            .filter(move |event| {
                let ours = matches!(event, CentralEvent::DeviceDisconnected(other) if *other == id);
                async move { ours }
            })
            .boxed()
            .into_future();
        Ok(values.take_until(disconnected).boxed())
    }

    async fn disconnect(&self) -> Result<(), TransportError> {
        self.peripheral.disconnect().await.map_err(map_err)
    }
}

/// Maps btleplug errors onto [`TransportError`].
#[allow(
    clippy::wildcard_enum_match_arm,
    reason = "any btleplug error variant added later belongs in `Backend`"
)]
fn map_err(err: ::btleplug::Error) -> TransportError {
    match err {
        ::btleplug::Error::PermissionDenied => TransportError::PermissionDenied,
        ::btleplug::Error::NotConnected => TransportError::NotConnected,
        ::btleplug::Error::TimedOut(_) => TransportError::Timeout,
        ::btleplug::Error::NoAdapterAvailable => TransportError::NoAdapter,
        other => TransportError::Backend(Box::new(other)),
    }
}
