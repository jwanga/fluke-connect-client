//! Built-in Bluetooth transport backed by [btleplug](https://crates.io/crates/btleplug).
//!
//! [`Adapter`] discovers Fluke Connect devices and connects to them,
//! producing a [`FlukeDevice`] over a [`BtleplugTransport`]. The only
//! btleplug types in this crate's own signatures are the adapter accepted by
//! [`Adapter::from_btleplug`] and the peripheral identifier accepted by
//! [`Adapter::describe`] and [`Adapter::connect_id`]; everything else is
//! wrapped so the backend can evolve independently.
//!
//! Discovery comes in two modes. The `find_*` and `scan` methods run their
//! own scan on the adapter. The `watch_*` methods and
//! [`PassiveAddressConnector`] only listen to the adapter's event stream, for
//! hosts that share the adapter and keep their own scan running; see
//! [`Adapter::from_btleplug`].

use core::future::Future;
use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use ::btleplug::api::{
    Central, CentralEvent, CentralState, Characteristic, Manager as _, Peripheral as _, ScanFilter,
    WriteType,
};
use ::btleplug::platform::{self, Manager, PeripheralId};
use futures_util::StreamExt as _;
use uuid::Uuid;

use crate::client::FlukeDevice;
use crate::error::{Error, Result};
use crate::protocol::MeasurementNotification;
use crate::protocol::uuids::READING_SERVICE;
use crate::reconnect::{Connector, Measurements, ReconnectPolicy, Reconnecting, Source};
use crate::transport::{BoxStream, Notification, Transport, TransportError};

/// How long [`Adapter::connect`] waits for the GATT connection.
///
/// The ir3000 FC advertises roughly every 10 seconds and a connection can
/// only start on an advertisement, so this allows for several intervals.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// A Fluke Connect device seen while scanning.
///
/// Values come from [`Adapter::scan`], the `find_*` and `watch_*` finders, or
/// [`Adapter::describe`] for a peripheral the host discovered itself; there
/// is no other constructor, so every device has been checked to advertise
/// the Fluke reading service.
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
    /// Applications that already hold a btleplug adapter should wrap it with
    /// [`from_btleplug`](Self::from_btleplug) instead of opening a second one.
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

    /// Wraps a btleplug adapter the application already owns.
    ///
    /// Use this instead of [`open`](Self::open) when the host has its own
    /// btleplug [`Manager`], for example because it also talks to other
    /// peripherals or chose among several adapters. The adapter must come
    /// from the btleplug version this crate links against, which
    /// [`fluke_connect_client::btleplug`](crate::btleplug) re-exports.
    ///
    /// The adapter stays shared, so this crate's use of it is visible to the
    /// rest of the application. Two modes are available:
    ///
    /// - **Owned scan**: [`scan`](Self::scan), [`find_first`](Self::find_first),
    ///   [`find_by_address`](Self::find_by_address),
    ///   [`connect_first`](Self::connect_first) and the reconnecting streams
    ///   ([`stream_with_reconnect`](Self::stream_with_reconnect),
    ///   [`measurements_with_reconnect`](Self::measurements_with_reconnect))
    ///   first stop any scan already running on the adapter, start their own
    ///   filtered on the Fluke reading service, and stop that again when
    ///   their window ends; the host's scan is not resumed. Before every
    ///   re-scan the reconnecting streams also forget the adapter's cached
    ///   peripherals, for *all* devices and not only the Fluke one, except
    ///   on Linux where `BlueZ` makes that a no-op; btleplug on
    ///   `CoreBluetooth` cannot reconnect through a stale handle otherwise.
    /// - **Passive**: the host owns the scan and this crate never touches
    ///   discovery. [`describe`](Self::describe) and
    ///   [`connect_id`](Self::connect_id) accept a peripheral the host found
    ///   itself; [`watch_first`](Self::watch_first) and
    ///   [`watch_by_address`](Self::watch_by_address) wait for the device on
    ///   the adapter's event stream; and [`PassiveAddressConnector`] gives
    ///   [`Reconnecting`] the same behaviour. None of them call
    ///   `start_scan`, `stop_scan` or `clear_peripherals`. The host must keep
    ///   a scan running whenever the device has to be (re)found, and on
    ///   `BlueZ` that scan's filter must either be empty or include the Fluke
    ///   reading service UUID ([`protocol::uuids::READING_SERVICE`]), because
    ///   `BlueZ` merges the filters of all discovery clients. `BlueZ` also
    ///   reports a device it still knows from its cache at once, scanning or
    ///   not. On `CoreBluetooth` btleplug 0.13 keeps a disconnected
    ///   peripheral's stale handle until `clear_peripherals` is called, so
    ///   passive reconnection there needs the host to clear the cache after
    ///   each disconnect; see [`PassiveAddressConnector`].
    ///
    /// [`protocol::uuids::READING_SERVICE`]: crate::protocol::uuids::READING_SERVICE
    ///
    /// ```no_run
    /// use fluke_connect_client::backend::Adapter;
    /// use fluke_connect_client::btleplug::api::Manager as _;
    /// use fluke_connect_client::btleplug::platform::Manager;
    ///
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let manager = Manager::new().await?;
    /// let first = manager.adapters().await?.into_iter().next().ok_or("no adapter")?;
    /// let adapter = Adapter::from_btleplug(first);
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub const fn from_btleplug(adapter: platform::Adapter) -> Self {
        Self { inner: adapter }
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
        scan_until(&self.inner, timeout, |_| false).await
    }

    /// Scans until the first Fluke Connect device appears or `timeout`
    /// elapses.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] if nothing was seen in time.
    pub async fn find_first(&self, timeout: Duration) -> Result<DiscoveredDevice> {
        first(scan_until(&self.inner, timeout, |_| true).await?)
    }

    /// Scans until the device with this [`address`](DiscoveredDevice::address)
    /// appears or `timeout` elapses, returning `None` if it was not seen.
    ///
    /// # Errors
    ///
    /// Returns an error if scanning cannot be started.
    pub async fn find_by_address(
        &self,
        address: &str,
        timeout: Duration,
    ) -> Result<Option<DiscoveredDevice>> {
        let found = scan_until(&self.inner, timeout, |d| d.address() == address).await?;
        Ok(by_address(found, address))
    }

    /// Waits for the first Fluke Connect device to advertise on a scan the
    /// host is already running, giving up after `window`.
    ///
    /// Passive counterpart of [`find_first`](Self::find_first): this only
    /// subscribes to the adapter's events and never starts or stops a scan,
    /// so nothing new is seen unless the host is scanning. On `BlueZ` the
    /// host's scan filter must be empty or include the Fluke reading service
    /// UUID, and a device the daemon still knows is reported at once from its
    /// cache. See [`from_btleplug`](Self::from_btleplug).
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] if nothing advertised in time, or an
    /// error if the event stream cannot be opened.
    pub async fn watch_first(&self, window: Duration) -> Result<DiscoveredDevice> {
        first(watch_until(&self.inner, window, |_| true).await?)
    }

    /// Waits for the device with this [`address`](DiscoveredDevice::address)
    /// to advertise on a scan the host is already running, returning `None`
    /// if it was not seen within `window`.
    ///
    /// Passive counterpart of [`find_by_address`](Self::find_by_address);
    /// see [`watch_first`](Self::watch_first) for what the host must do.
    ///
    /// # Errors
    ///
    /// Returns an error if the event stream cannot be opened.
    pub async fn watch_by_address(
        &self,
        address: &str,
        window: Duration,
    ) -> Result<Option<DiscoveredDevice>> {
        let found = watch_until(&self.inner, window, |d| d.address() == address).await?;
        Ok(by_address(found, address))
    }

    /// Builds a [`DiscoveredDevice`] for a peripheral the host discovered
    /// itself, or `None` if it does not advertise the Fluke reading service.
    ///
    /// This is the way into the crate for a host that owns the scan: pass the
    /// [`PeripheralId`] from its own `DeviceDiscovered` event (or from
    /// `Central::peripherals`) and connect to the result with
    /// [`connect`](Self::connect). No scan is started or stopped.
    ///
    /// # Errors
    ///
    /// Returns an error if the adapter does not know the peripheral or cannot
    /// read its advertisement properties.
    pub async fn describe(&self, id: &PeripheralId) -> Result<Option<DiscoveredDevice>> {
        Discovery::describe(&self.inner, id).await
    }

    /// [`describe`](Self::describe) followed by
    /// [`connect_with_timeout`](Self::connect_with_timeout).
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] if the peripheral does not advertise the
    /// Fluke reading service, or a transport error if the connection fails.
    pub async fn connect_id(
        &self,
        id: &PeripheralId,
        timeout: Duration,
    ) -> Result<FlukeDevice<BtleplugTransport>> {
        let device = self.describe(id).await?.ok_or(Error::NotFound)?;
        self.connect_with_timeout(&device, timeout).await
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

    /// Streams `source` from `device`, re-scanning and reconnecting whenever
    /// the connection drops. See the [`reconnect`](crate::reconnect) module.
    ///
    /// The first attempt connects to `device` directly; every later attempt
    /// forgets cached peripherals (except on Linux, where that is a no-op)
    /// and scans for the device's address again, which is what btleplug
    /// needs after a disconnect. A host that shares the adapter and owns the
    /// scan should build the stream from a [`PassiveAddressConnector`] with
    /// [`Reconnecting::new`] instead.
    ///
    /// # Panics
    ///
    /// Panics if called outside a Tokio runtime.
    pub fn stream_with_reconnect<S: Source<BtleplugTransport>>(
        &self,
        device: &DiscoveredDevice,
        source: S,
        policy: ReconnectPolicy,
    ) -> Reconnecting<S::Item> {
        let connector =
            AddressConnector(PassiveAddressConnector::new(self.clone(), &device.address));
        Reconnecting::new(connector, source, Some(device.clone()), policy)
    }

    /// [`stream_with_reconnect`](Self::stream_with_reconnect) over the
    /// auto-selecting measurement stream ([`Measurements`]); the choice for
    /// most applications.
    ///
    /// # Panics
    ///
    /// Panics if called outside a Tokio runtime.
    pub fn measurements_with_reconnect(
        &self,
        device: &DiscoveredDevice,
        policy: ReconnectPolicy,
    ) -> Reconnecting<Result<MeasurementNotification>> {
        self.stream_with_reconnect(device, Measurements, policy)
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
        self.connect_with_timeout(device, CONNECT_TIMEOUT).await
    }

    /// Connects to a discovered device with an explicit connection timeout.
    ///
    /// # Errors
    ///
    /// Returns a transport error if the connection or service discovery
    /// fails, or [`TransportError::Timeout`] if the link is not up in time.
    pub async fn connect_with_timeout(
        &self,
        device: &DiscoveredDevice,
        timeout: Duration,
    ) -> Result<FlukeDevice<BtleplugTransport>> {
        let peripheral = self.inner.peripheral(&device.id).await.map_err(map_err)?;
        #[cfg(feature = "tracing")]
        tracing::debug!(device = %device, "connecting");
        // If this future is dropped mid-attempt (a reconnecting stream being
        // stopped), the OS may still complete the connect later and hold the
        // device with nothing attached; the guard releases it.
        let mut guard = ConnectGuard {
            peripheral: Some(peripheral.clone()),
        };
        let attempt = async {
            peripheral.connect_with_timeout(timeout).await?;
            peripheral.discover_services().await
        };
        let outcome = tokio::time::timeout(timeout, attempt)
            .await
            .map_err(|_| TransportError::Timeout)
            .and_then(|r| r.map_err(map_err));
        guard.peripheral = None;
        if let Err(e) = outcome {
            // A timed-out or failed attempt can leave the link half-open on
            // the OS side; releasing it is best effort.
            let _ = peripheral.disconnect().await;
            return Err(e.into());
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
}

impl From<platform::Adapter> for Adapter {
    /// Same as [`Adapter::from_btleplug`].
    fn from(adapter: platform::Adapter) -> Self {
        Self::from_btleplug(adapter)
    }
}

/// What the discovery loops need from a Bluetooth central.
///
/// Implemented by [`platform::Adapter`] and, in unit tests, by a fake that
/// counts scan calls, so the owned and passive loops can be exercised on
/// every operating system without hardware.
trait Discovery: Send + Sync {
    /// Identifies a peripheral in events.
    type Id: Clone + PartialEq + Send + 'static;
    /// An event from the central.
    type Event: Send + 'static;
    /// What [`describe`](Self::describe) produces.
    type Device: Send;

    /// Subscribes to the central's events.
    fn events(&self) -> impl Future<Output = Result<BoxStream<'static, Self::Event>>> + Send;

    /// The peripheral an advertisement event concerns; `None` for any other
    /// event.
    fn advertised(event: Self::Event) -> Option<Self::Id>;

    /// Builds a device if the peripheral advertises the Fluke reading
    /// service.
    fn describe(&self, id: &Self::Id) -> impl Future<Output = Result<Option<Self::Device>>> + Send;

    /// Starts a scan filtered on the Fluke reading service.
    fn start_scan(&self) -> impl Future<Output = Result<()>> + Send;

    /// Stops the running scan, if any.
    fn stop_scan(&self) -> impl Future<Output = Result<()>> + Send;
}

impl Discovery for platform::Adapter {
    type Id = PeripheralId;
    type Event = CentralEvent;
    type Device = DiscoveredDevice;

    async fn events(&self) -> Result<BoxStream<'static, CentralEvent>> {
        Ok(Central::events(self).await.map_err(map_err)?)
    }

    fn advertised(event: CentralEvent) -> Option<PeripheralId> {
        match event {
            CentralEvent::DeviceDiscovered(id)
            | CentralEvent::DeviceUpdated(id)
            | CentralEvent::ServicesAdvertisement { id, .. } => Some(id),
            CentralEvent::DeviceConnected(_)
            | CentralEvent::DeviceDisconnected(_)
            | CentralEvent::DeviceServicesModified(_)
            | CentralEvent::ManufacturerDataAdvertisement { .. }
            | CentralEvent::ServiceDataAdvertisement { .. }
            | CentralEvent::RssiUpdate { .. }
            | CentralEvent::StateUpdate(_) => None,
        }
    }

    async fn describe(&self, id: &PeripheralId) -> Result<Option<DiscoveredDevice>> {
        let peripheral = self.peripheral(id).await.map_err(map_err)?;
        let Some(props) = peripheral.properties().await.map_err(map_err)? else {
            return Ok(None);
        };
        if !props.services.contains(&Uuid::from_u128(READING_SERVICE)) {
            return Ok(None);
        }
        // CoreBluetooth hides MAC addresses and btleplug reports all zeros;
        // fall back to the platform's peripheral identifier there.
        let address = if props.address.into_inner() == [0; 6] {
            id.to_string()
        } else {
            props.address.to_string()
        };
        Ok(Some(DiscoveredDevice {
            id: id.clone(),
            name: props.local_name,
            address,
            rssi: props.rssi,
        }))
    }

    async fn start_scan(&self) -> Result<()> {
        let filter = ScanFilter {
            services: vec![Uuid::from_u128(READING_SERVICE)],
        };
        Ok(Central::start_scan(self, filter).await.map_err(map_err)?)
    }

    async fn stop_scan(&self) -> Result<()> {
        Ok(Central::stop_scan(self).await.map_err(map_err)?)
    }
}

/// Owned-scan loop: stops any running scan, starts one filtered on the Fluke
/// reading service, collects devices for `window` (or until `stop` returns
/// true for one) and stops the scan again. The advertisement is checked a
/// second time in [`Discovery::describe`] because `BlueZ` merges scan
/// filters from all D-Bus clients. A scan abandoned mid-window (the future
/// dropped) is stopped by the next scan's pre-start `stop_scan`.
async fn scan_until<D: Discovery>(
    central: &D,
    window: Duration,
    stop: impl Fn(&D::Device) -> bool + Send,
) -> Result<Vec<D::Device>> {
    let events = central.events().await?;
    // `BlueZ` rejects start_scan while a previous scan is still running.
    let _ = central.stop_scan().await;
    central.start_scan().await?;
    let found = collect(central, events, window, stop).await;
    // Stopping the scan is best effort; a failure here must not hide results.
    let _ = central.stop_scan().await;
    Ok(found)
}

/// Passive loop: like [`scan_until`] but relies on a scan the host is
/// running and never touches discovery.
async fn watch_until<D: Discovery>(
    central: &D,
    window: Duration,
    stop: impl Fn(&D::Device) -> bool + Send,
) -> Result<Vec<D::Device>> {
    let events = central.events().await?;
    Ok(collect(central, events, window, stop).await)
}

/// Describes every peripheral that advertises on `events` until `window`
/// elapses, the stream ends, or `stop` accepts a device. Each peripheral is
/// reported once; one that fails to describe is retried on its next event.
async fn collect<D: Discovery>(
    central: &D,
    mut events: BoxStream<'static, D::Event>,
    window: Duration,
    stop: impl Fn(&D::Device) -> bool + Send,
) -> Vec<D::Device> {
    let mut found: Vec<(D::Id, D::Device)> = Vec::new();
    let deadline = tokio::time::sleep(window);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            () = &mut deadline => break,
            event = events.next() => {
                let Some(event) = event else { break };
                let Some(id) = D::advertised(event) else { continue };
                if found.iter().any(|(seen, _)| *seen == id) {
                    continue;
                }
                if let Ok(Some(device)) = central.describe(&id).await {
                    let done = stop(&device);
                    found.push((id, device));
                    if done {
                        break;
                    }
                }
            }
        }
    }
    found.into_iter().map(|(_, device)| device).collect()
}

/// The first device found, or [`Error::NotFound`].
fn first(found: Vec<DiscoveredDevice>) -> Result<DiscoveredDevice> {
    found.into_iter().next().ok_or(Error::NotFound)
}

/// The found device with this address, if any.
fn by_address(found: Vec<DiscoveredDevice>, address: &str) -> Option<DiscoveredDevice> {
    found.into_iter().find(|d| d.address() == address)
}

/// Disconnects a peripheral whose connect attempt was abandoned mid-flight.
struct ConnectGuard {
    /// The peripheral being connected, until the attempt completes.
    peripheral: Option<platform::Peripheral>,
}

impl Drop for ConnectGuard {
    fn drop(&mut self) {
        let Some(peripheral) = self.peripheral.take() else {
            return;
        };
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = peripheral.disconnect().await;
            });
        }
    }
}

/// [`Connector`] that re-finds one device by address on every attempt with
/// its own scan; the owned-scan twin of [`PassiveAddressConnector`].
#[derive(Debug)]
struct AddressConnector(PassiveAddressConnector);

impl Connector for AddressConnector {
    type Target = DiscoveredDevice;
    type Transport = BtleplugTransport;

    async fn find(&self, window: Duration) -> Result<Option<DiscoveredDevice>> {
        // After a disconnect only a freshly scanned handle works: on
        // `CoreBluetooth` btleplug otherwise keeps a stale one whose
        // notification stream stays silent forever. Clearing affects every
        // device the adapter knows, which is why this stays private. On
        // Linux btleplug's `BlueZ` backend implements it as a no-op, so the
        // call is skipped there.
        if !cfg!(target_os = "linux") {
            Central::clear_peripherals(&self.0.adapter.inner)
                .await
                .map_err(map_err)?;
        }
        self.0
            .adapter
            .find_by_address(&self.0.address, window)
            .await
    }

    async fn connect(
        &self,
        target: &DiscoveredDevice,
        timeout: Duration,
    ) -> Result<FlukeDevice<BtleplugTransport>> {
        self.0.connect(target, timeout).await
    }
}

/// [`Connector`] for a host that owns the scan on a shared adapter.
///
/// [`find`](Connector::find) is [`Adapter::watch_by_address`] and
/// [`connect`](Connector::connect) is [`Adapter::connect_with_timeout`], so
/// the reconnecting stream never starts or stops a scan and never clears the
/// adapter's peripheral cache. The host must keep its scan running while the
/// stream may need to reconnect; see [`Adapter::from_btleplug`] for the
/// `BlueZ` filter rule. Pair it with [`Reconnecting::new`]:
///
/// ```no_run
/// use std::time::Duration;
///
/// use fluke_connect_client::backend::{Adapter, PassiveAddressConnector};
/// use fluke_connect_client::reconnect::{Measurements, ReconnectPolicy, Reconnecting};
///
/// # async fn run(adapter: Adapter) -> Result<(), Box<dyn std::error::Error>> {
/// // The host is already scanning on the adapter it shared with us.
/// let device = adapter.watch_first(Duration::from_secs(60)).await?;
/// let connector = PassiveAddressConnector::new(adapter, device.address());
/// let stream = Reconnecting::new(
///     connector,
///     Measurements,
///     Some(device),
///     ReconnectPolicy::default(),
/// );
/// # let _ = stream;
/// # Ok(()) }
/// ```
///
/// On `CoreBluetooth` btleplug 0.13 keeps a disconnected peripheral's stale
/// handle in its cache and ignores the device's next advertisement, so a
/// passive reconnection there connects through the old handle and receives
/// no notifications. Until that is fixed upstream, a macOS host must call
/// `clear_peripherals` on its adapter after each disconnect (which forgets
/// every device, not only this one) or use
/// [`Adapter::stream_with_reconnect`], which does so itself.
#[derive(Debug, Clone)]
pub struct PassiveAddressConnector {
    /// Adapter to watch and connect with.
    adapter: Adapter,
    /// Platform address of the device to follow.
    address: String,
}

impl PassiveAddressConnector {
    /// Follows the device with this [`address`](DiscoveredDevice::address)
    /// on `adapter`.
    #[must_use]
    pub fn new(adapter: Adapter, address: impl Into<String>) -> Self {
        Self {
            adapter,
            address: address.into(),
        }
    }
}

impl Connector for PassiveAddressConnector {
    type Target = DiscoveredDevice;
    type Transport = BtleplugTransport;

    async fn find(&self, window: Duration) -> Result<Option<DiscoveredDevice>> {
        self.adapter.watch_by_address(&self.address, window).await
    }

    async fn connect(
        &self,
        target: &DiscoveredDevice,
        timeout: Duration,
    ) -> Result<FlukeDevice<BtleplugTransport>> {
        self.adapter.connect_with_timeout(target, timeout).await
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
        let disconnected = Central::events(&self.adapter)
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

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::unused_async_trait_impl,
    reason = "tests may fail loudly; the fake implements async trait methods synchronously"
)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use futures_util::StreamExt as _;
    use futures_util::stream;
    use tokio::time::Instant;

    use super::{Discovery, Result, scan_until, watch_until};
    use crate::transport::BoxStream;

    /// The three advertisement events the loops act on, plus one they must
    /// ignore, mirroring the `CentralEvent` variants `advertised` accepts.
    #[derive(Debug, Clone, Copy)]
    enum Event {
        /// `CentralEvent::DeviceDiscovered`.
        Discovered(u8),
        /// `CentralEvent::DeviceUpdated`.
        Updated(u8),
        /// `CentralEvent::ServicesAdvertisement`.
        Services(u8),
        /// Any event that is not an advertisement.
        Other,
    }

    /// Central that replays a fixed event list and counts scan calls.
    struct FakeCentral {
        /// Events to replay, taken on the first `events` call.
        events: Mutex<Option<Vec<Event>>>,
        /// Peripherals that advertise the Fluke service, by id, with their
        /// address.
        fluke: HashMap<u8, &'static str>,
        /// `start_scan` calls.
        starts: AtomicUsize,
        /// `stop_scan` calls.
        stops: AtomicUsize,
    }

    impl FakeCentral {
        /// A central that will emit `events`; peripherals listed in `fluke`
        /// advertise the reading service.
        fn new(events: &[Event], fluke: &[(u8, &'static str)]) -> Self {
            Self {
                events: Mutex::new(Some(events.to_vec())),
                fluke: fluke.iter().copied().collect(),
                starts: AtomicUsize::new(0),
                stops: AtomicUsize::new(0),
            }
        }

        /// `(start_scan, stop_scan)` call counts.
        fn scan_calls(&self) -> (usize, usize) {
            (
                self.starts.load(Ordering::SeqCst),
                self.stops.load(Ordering::SeqCst),
            )
        }
    }

    impl Discovery for FakeCentral {
        type Id = u8;
        type Event = Event;
        type Device = String;

        async fn events(&self) -> Result<BoxStream<'static, Event>> {
            let events = self.events.lock().unwrap().take().unwrap_or_default();
            // A real event stream stays open after the recorded events, so
            // the window end is what ends the loop.
            Ok(stream::iter(events).chain(stream::pending()).boxed())
        }

        fn advertised(event: Event) -> Option<u8> {
            match event {
                Event::Discovered(id) | Event::Updated(id) | Event::Services(id) => Some(id),
                Event::Other => None,
            }
        }

        async fn describe(&self, id: &u8) -> Result<Option<String>> {
            Ok(self.fluke.get(id).map(|address| (*address).to_owned()))
        }

        async fn start_scan(&self) -> Result<()> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn stop_scan(&self) -> Result<()> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// One scan window in the tests.
    const WINDOW: Duration = Duration::from_secs(30);

    /// What `PassiveAddressConnector::find` and `Adapter::watch_by_address`
    /// run: the passive loop stopped by an address match.
    async fn passive_find(central: &FakeCentral, address: &str) -> Result<Option<String>> {
        let found = watch_until(central, WINDOW, |d| d == address).await?;
        Ok(found.into_iter().find(|d| d == address))
    }

    #[tokio::test(start_paused = true)]
    async fn passive_find_returns_the_device_on_every_advertisement_event() {
        for event in [Event::Discovered(1), Event::Updated(1), Event::Services(1)] {
            let central = FakeCentral::new(&[Event::Other, event], &[(1, "AA:BB")]);
            let started = Instant::now();
            let found = passive_find(&central, "AA:BB").await.unwrap();
            assert_eq!(found.as_deref(), Some("AA:BB"), "{event:?}");
            assert!(
                started.elapsed() < WINDOW,
                "{event:?} should end the window early"
            );
            assert_eq!(central.scan_calls(), (0, 0), "{event:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn passive_find_returns_none_at_the_window_end_without_scanning() {
        // Peripheral 2 advertises but is not a Fluke device; 1 never shows up.
        let central = FakeCentral::new(&[Event::Discovered(2), Event::Updated(2)], &[(1, "AA:BB")]);
        let started = Instant::now();
        let found = passive_find(&central, "AA:BB").await.unwrap();
        assert!(found.is_none());
        assert_eq!(started.elapsed(), WINDOW);
        assert_eq!(central.scan_calls(), (0, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn passive_find_ignores_other_fluke_devices() {
        let central = FakeCentral::new(
            &[Event::Discovered(1), Event::Discovered(2)],
            &[(1, "AA:BB"), (2, "CC:DD")],
        );
        let found = passive_find(&central, "CC:DD").await.unwrap();
        assert_eq!(found.as_deref(), Some("CC:DD"));
        assert_eq!(central.scan_calls(), (0, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn watch_first_skips_peripherals_without_the_service() {
        let central = FakeCentral::new(
            &[
                Event::Discovered(9),
                Event::Services(9),
                Event::Discovered(1),
            ],
            &[(1, "AA:BB")],
        );
        let found = watch_until(&central, WINDOW, |_| true).await.unwrap();
        assert_eq!(found, vec!["AA:BB".to_owned()]);
        assert_eq!(central.scan_calls(), (0, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn passive_collect_reports_each_device_once() {
        let central = FakeCentral::new(
            &[Event::Discovered(1), Event::Updated(1), Event::Services(1)],
            &[(1, "AA:BB")],
        );
        let found = watch_until(&central, WINDOW, |_| false).await.unwrap();
        assert_eq!(found, vec!["AA:BB".to_owned()]);
    }

    #[tokio::test(start_paused = true)]
    async fn owned_scan_starts_and_stops_the_scan() {
        let central = FakeCentral::new(&[Event::Discovered(1)], &[(1, "AA:BB")]);
        let found = scan_until(&central, WINDOW, |_| true).await.unwrap();
        assert_eq!(found, vec!["AA:BB".to_owned()]);
        // stop before start (BlueZ rejects a second start) and stop at the end.
        assert_eq!(central.scan_calls(), (1, 2));
    }
}
