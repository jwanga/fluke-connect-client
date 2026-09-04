//! A device stream that survives disconnects.
//!
//! [`Reconnecting`] runs a supervisor task that finds the device, connects,
//! opens a [`Source`] on it and forwards its items, starting over whenever
//! the connection is lost: it re-scans in fixed windows (the device sets the
//! pace by advertising), retries failed connections with exponential
//! backoff, and never gives up unless [`ReconnectPolicy::max_attempts`]
//! says so. An optional [`ReconnectPolicy::idle_timeout`] treats a link
//! that stays up but goes silent as lost. [`Readings`], [`Measurements`], [`AsciiReadings`] and
//! [`BatteryUpdates`] are sources for the corresponding [`FlukeDevice`]
//! subscriptions.
//!
//! Every reconnection goes through a fresh scan on purpose. With btleplug on
//! `CoreBluetooth`, a peripheral handle that has disconnected once is dead:
//! its notification stream stays silent forever and connecting through it
//! fails. Only a handle produced by a new scan works, so
//! [`Connector::find`] is called before every reconnection attempt.
//!
//! The engine is generic over [`Connector`] and [`Source`], so the policy
//! can be tested without hardware; the built-in Bluetooth backend provides a
//! connector through `backend::Adapter::stream_with_reconnect` and its
//! `measurements_with_reconnect` shorthand.

use core::convert::Infallible;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::hash::{BuildHasher as _, Hasher as _, RandomState};
use std::time::Duration;

use futures_util::{Stream, StreamExt as _};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::client::FlukeDevice;
use crate::error::{Error, Result};
use crate::protocol::{AsciiReading, MeasurementNotification, ReadingNotification};
use crate::transport::{BoxStream, Transport};

/// Finds and connects to a Fluke Connect device on behalf of
/// [`Reconnecting`].
///
/// Implementations must return a *fresh* connection each time; reusing a
/// peripheral handle from before a disconnect does not work with btleplug.
pub trait Connector: Send + Sync + 'static {
    /// What [`find`](Self::find) produces and [`connect`](Self::connect)
    /// consumes, typically a discovered device.
    type Target: Send + 'static;
    /// Transport of the devices this connector produces.
    type Transport: Transport + 'static;

    /// Looks for the device for about `window`.
    ///
    /// Returns `Ok(None)` if it was not seen in time; the engine calls again
    /// at once, so the window itself is the pacing. Errors are treated like
    /// connection failures and backed off.
    fn find(&self, window: Duration) -> impl Future<Output = Result<Option<Self::Target>>> + Send;

    /// Connects to `target` and discovers services, giving up after `timeout`.
    fn connect(
        &self,
        target: &Self::Target,
        timeout: Duration,
    ) -> impl Future<Output = Result<FlukeDevice<Self::Transport>>> + Send;
}

/// What a [`Reconnecting`] stream subscribes to on every connection.
///
/// [`open`](Self::open) is called once per successful connection, after
/// service discovery, and its stream is forwarded as [`Event::Item`]s until
/// it ends, which the engine takes to mean the link is gone. An error from
/// `open` counts as a failed connection attempt: the device is disconnected,
/// the failure is backed off and the next attempt goes through a fresh scan.
///
/// The ready-made sources ([`Readings`], [`Measurements`],
/// [`AsciiReadings`], [`BatteryUpdates`]) are zero-sized and cover every
/// subscription [`FlukeDevice`] offers; implement the trait yourself to
/// combine or transform them.
pub trait Source<T: Transport>: Send + Sync + 'static {
    /// What the opened stream yields.
    type Item: Send + 'static;

    /// Subscribes on a freshly connected device.
    ///
    /// The returned stream must end when the connection is lost, as the
    /// [`FlukeDevice`] streams do.
    fn open(
        &self,
        device: &FlukeDevice<T>,
    ) -> impl Future<Output = Result<BoxStream<'static, Self::Item>>> + Send;
}

/// [`Source`] for [`FlukeDevice::readings`]: the binary reading record.
#[derive(Debug, Clone, Copy, Default)]
pub struct Readings;

impl<T: Transport> Source<T> for Readings {
    type Item = Result<ReadingNotification>;

    async fn open(&self, device: &FlukeDevice<T>) -> Result<BoxStream<'static, Self::Item>> {
        device.readings().await
    }
}

/// [`Source`] for [`FlukeDevice::measurements`]: whichever reading
/// characteristic the device exposes.
///
/// Locks onto the binary record once it notifies. Fails with
/// [`Error::NoReadingCharacteristic`] on a device with neither
/// characteristic, which the engine retries like any other connect failure.
#[derive(Debug, Clone, Copy, Default)]
pub struct Measurements;

impl<T: Transport> Source<T> for Measurements {
    type Item = Result<MeasurementNotification>;

    async fn open(&self, device: &FlukeDevice<T>) -> Result<BoxStream<'static, Self::Item>> {
        device.measurements().await
    }
}

/// [`Source`] for [`FlukeDevice::ascii_readings`]: the ASCII display string.
#[derive(Debug, Clone, Copy, Default)]
pub struct AsciiReadings;

impl<T: Transport> Source<T> for AsciiReadings {
    type Item = Result<AsciiReading>;

    async fn open(&self, device: &FlukeDevice<T>) -> Result<BoxStream<'static, Self::Item>> {
        device.ascii_readings().await
    }
}

/// [`Source`] for [`FlukeDevice::battery_updates`]: battery level in percent.
#[derive(Debug, Clone, Copy, Default)]
pub struct BatteryUpdates;

impl<T: Transport> Source<T> for BatteryUpdates {
    type Item = u8;

    async fn open(&self, device: &FlukeDevice<T>) -> Result<BoxStream<'static, Self::Item>> {
        device.battery_updates().await
    }
}

/// Tunables for [`Reconnecting`].
///
/// The struct is non-exhaustive, so downstream crates set fields on a
/// default value rather than using struct-update syntax:
///
/// ```
/// use fluke_connect_client::reconnect::ReconnectPolicy;
///
/// let mut policy = ReconnectPolicy::default();
/// policy.max_attempts = Some(20);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReconnectPolicy {
    /// Length of one scan window (default 30 s).
    pub scan_window: Duration,
    /// Time allowed for connect plus service discovery (default 30 s).
    pub connect_timeout: Duration,
    /// Connect attempts against one found device before scanning again
    /// (default 5).
    pub connect_attempts_per_scan: u32,
    /// Delay before the first connect retry (default 1 s); doubles per retry.
    pub initial_backoff: Duration,
    /// Upper bound on the retry delay before jitter (default 15 s).
    pub max_backoff: Duration,
    /// Consecutive failures (empty scan windows, scan errors and failed
    /// connects; reset by a successful connection) after which the stream
    /// yields [`Event::GaveUp`] and ends. `None` never gives up.
    pub max_attempts: Option<u32>,
    /// Longest silence tolerated on a connected link. When no item arrives
    /// within it the link is treated as lost: the device is disconnected,
    /// [`Event::Disconnected`] follows and the stream reconnects through a
    /// fresh scan. `None` (the default) waits forever, so a meter left in
    /// HOLD or a slow logging interval is never kicked; set it when the
    /// consumer would rather reconnect than trust a link that has gone
    /// quiet, which `CoreBluetooth` is known to leave standing.
    pub idle_timeout: Option<Duration>,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            scan_window: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(30),
            connect_attempts_per_scan: 5,
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(15),
            max_attempts: None,
            idle_timeout: None,
        }
    }
}

impl ReconnectPolicy {
    /// Jittered delay before retry number `retry` (1-based): uniform in
    /// `[0, min(initial_backoff * 2^(retry - 1), max_backoff)]`.
    fn backoff(&self, retry: u32) -> Duration {
        let factor = 2_u32.saturating_pow(retry.saturating_sub(1));
        let cap = self
            .initial_backoff
            .saturating_mul(factor)
            .min(self.max_backoff);
        let cap_ms = u64::try_from(cap.as_millis()).unwrap_or(u64::MAX);
        // `RandomState` is randomly seeded per instance, which is all the
        // randomness jitter needs; it avoids a dependency on a rand crate.
        let roll = RandomState::new().build_hasher().finish();
        Duration::from_millis(roll.checked_rem(cap_ms.saturating_add(1)).unwrap_or(0))
    }
}

/// One step in the life of a reconnecting stream.
///
/// `I` is the [`Source`]'s item type.
#[derive(Debug)]
#[non_exhaustive]
pub enum Event<I> {
    /// Connected and subscribed; items follow.
    Connected,
    /// An item from the source. For the reading sources this is a decoded
    /// reading or a decode error; either way the connection is kept.
    Item(I),
    /// The connection was lost; a new scan starts.
    Disconnected,
    /// A scan window passed without seeing the device; scanning again.
    WaitingForDevice,
    /// A connect attempt or a scan failed; sleeping `delay` before the next one.
    Retrying {
        /// Consecutive failed attempts so far, counting this one.
        attempt: u32,
        /// How long the engine sleeps before retrying.
        delay: Duration,
        /// Why the attempt failed.
        error: Error,
    },
    /// [`ReconnectPolicy::max_attempts`] was reached; this is the last event.
    GaveUp {
        /// Attempts made since the last successful connection.
        attempts: u32,
        /// The final failure ([`Error::NotFound`] after an empty scan window).
        last_error: Error,
    },
}

/// Stops a [`Reconnecting`] stream from anywhere.
#[derive(Debug, Clone)]
pub struct StopHandle {
    /// Shared stop flag.
    tx: watch::Sender<bool>,
}

impl StopHandle {
    /// Disconnects the current device, if any, and ends the stream.
    ///
    /// Idempotent and safe to call from any task or thread. The stream must
    /// keep being polled for the disconnect to complete.
    pub fn stop(&self) {
        self.tx.send_replace(true);
    }

    /// Whether [`stop`](Self::stop) has been called.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        *self.tx.borrow()
    }
}

/// A device stream that survives disconnects. See the [module docs](self).
///
/// `I` is the item type of the [`Source`] given to [`new`](Self::new).
/// Dropping it aborts the supervisor task *without* disconnecting the
/// device; prefer [`StopHandle::stop`] followed by draining the stream,
/// which disconnects cleanly.
#[derive(Debug)]
#[must_use = "streams do nothing unless polled"]
pub struct Reconnecting<I> {
    /// Events from the supervisor task.
    rx: mpsc::Receiver<Event<I>>,
    /// The supervisor task; aborted on drop.
    task: JoinHandle<()>,
    /// Stop flag shared with the handles given out by `stop_handle`.
    stop: StopHandle,
}

impl<I: Send + 'static> Reconnecting<I> {
    /// Starts supervising: connects, opens `source` and forwards its items,
    /// reconnecting whenever the link drops.
    ///
    /// `initial`, if given, is connected to before any scan; pass the device
    /// you just discovered.
    ///
    /// # Panics
    ///
    /// Panics if called outside a Tokio runtime.
    pub fn new<C, S>(
        connector: C,
        source: S,
        initial: Option<C::Target>,
        policy: ReconnectPolicy,
    ) -> Self
    where
        C: Connector,
        S: Source<C::Transport, Item = I>,
    {
        let (stop_tx, stop_rx) = watch::channel(false);
        let (tx, rx) = mpsc::channel(EVENT_BUFFER);
        let task = tokio::spawn(async move {
            let mut runner = Runner {
                connector,
                source,
                policy,
                tx,
                stop: stop_rx,
                attempts: 0,
            };
            let _exit = runner.run(initial).await;
        });
        Self {
            rx,
            task,
            stop: StopHandle { tx: stop_tx },
        }
    }

    /// A handle that stops this stream.
    #[must_use]
    pub fn stop_handle(&self) -> StopHandle {
        self.stop.clone()
    }
}

impl<I> Stream for Reconnecting<I> {
    type Item = Event<I>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Event<I>>> {
        self.get_mut().rx.poll_recv(cx)
    }
}

impl<I> Drop for Reconnecting<I> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Capacity of the event channel; the supervisor waits when it is full.
const EVENT_BUFFER: usize = 32;
/// Bound on the disconnect issued when stopping.
const STOP_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Why the supervisor loop returned.
#[derive(Debug)]
enum Exit {
    /// [`StopHandle::stop`] was called.
    Stopped,
    /// The [`Reconnecting`] stream was dropped.
    ConsumerGone,
    /// [`ReconnectPolicy::max_attempts`] was reached.
    GaveUp,
}

/// What waiting for the next item of a session produced.
enum Next<I> {
    /// An item arrived.
    Item(I),
    /// The source's stream ended.
    Ended,
    /// Nothing arrived within [`ReconnectPolicy::idle_timeout`].
    Idle,
}

/// Waits for the next item, giving up after `idle_timeout` of silence.
async fn next_item<I>(
    items: &mut BoxStream<'static, I>,
    idle_timeout: Option<Duration>,
) -> Next<I> {
    let Some(limit) = idle_timeout else {
        return items.next().await.map_or(Next::Ended, Next::Item);
    };
    match tokio::time::timeout(limit, items.next()).await {
        Ok(Some(item)) => Next::Item(item),
        Ok(None) => Next::Ended,
        Err(_elapsed) => Next::Idle,
    }
}

/// How a connected session ended.
enum Outcome {
    /// The source's stream ended: the link is gone.
    LinkLost,
    /// Opening the source failed after the link came up; the handle is dead.
    Failed(Error),
}

/// Resolves when stop is requested; pends forever if every [`StopHandle`]
/// is gone, so a dropped handle is never mistaken for a stop.
async fn stopped(rx: &mut watch::Receiver<bool>) {
    if rx.wait_for(|stop| *stop).await.is_err() {
        core::future::pending::<()>().await;
    }
}

/// Runs `work` unless stop is requested first.
async fn unless_stopped<F: Future>(
    stop: &mut watch::Receiver<bool>,
    work: F,
) -> core::result::Result<F::Output, Exit> {
    tokio::select! {
        biased;
        () = stopped(stop) => Err(Exit::Stopped),
        output = work => Ok(output),
    }
}

/// Disconnects a device on the way out, bounded in time and ignoring errors.
async fn disconnect_quietly<T: Transport>(device: &FlukeDevice<T>) {
    let _ = tokio::time::timeout(STOP_DISCONNECT_TIMEOUT, device.disconnect()).await;
}

/// State owned by the supervisor task.
struct Runner<C: Connector, S: Source<C::Transport>> {
    /// Finds and connects to the device.
    connector: C,
    /// What to subscribe to on each connection.
    source: S,
    /// Tunables.
    policy: ReconnectPolicy,
    /// Where events go.
    tx: mpsc::Sender<Event<S::Item>>,
    /// Stop flag.
    stop: watch::Receiver<bool>,
    /// Consecutive failures since the last successful connection.
    attempts: u32,
}

impl<C: Connector, S: Source<C::Transport>> Runner<C, S> {
    /// The supervisor loop: scan, connect with retries, stream, repeat.
    async fn run(
        &mut self,
        mut initial: Option<C::Target>,
    ) -> core::result::Result<Infallible, Exit> {
        loop {
            let target = match initial.take() {
                Some(target) => target,
                None => match self.find().await? {
                    Some(target) => target,
                    None => continue,
                },
            };
            for retry in 1..=self.policy.connect_attempts_per_scan.max(1) {
                let connect = self.connector.connect(&target, self.policy.connect_timeout);
                let device = match unless_stopped(&mut self.stop, connect).await? {
                    Ok(device) => device,
                    Err(error) => {
                        self.failure(retry, error).await?;
                        continue;
                    }
                };
                match self.session(&device).await {
                    Ok(Outcome::LinkLost) => {
                        self.emit(Event::Disconnected).await?;
                        break;
                    }
                    Ok(Outcome::Failed(error)) => {
                        // The handle is dead; back off, then go to a fresh scan.
                        self.failure(retry, error).await?;
                        break;
                    }
                    Err(exit) => {
                        disconnect_quietly(&device).await;
                        return Err(exit);
                    }
                }
            }
        }
    }

    /// Opens the source and forwards its items until the link drops.
    async fn session(
        &mut self,
        device: &FlukeDevice<C::Transport>,
    ) -> core::result::Result<Outcome, Exit> {
        let open = self.source.open(device);
        let mut items = match unless_stopped(&mut self.stop, open).await? {
            Ok(items) => items,
            Err(error) => {
                disconnect_quietly(device).await;
                return Ok(Outcome::Failed(error));
            }
        };
        self.attempts = 0;
        self.emit(Event::Connected).await?;
        let idle_timeout = self.policy.idle_timeout;
        loop {
            let next = next_item(&mut items, idle_timeout);
            match unless_stopped(&mut self.stop, next).await? {
                Next::Item(item) => self.emit(Event::Item(item)).await?,
                Next::Ended => return Ok(Outcome::LinkLost),
                Next::Idle => {
                    // The link may well be up, but nothing is coming through
                    // it; a fresh connection is the only cure.
                    disconnect_quietly(device).await;
                    return Ok(Outcome::LinkLost);
                }
            }
        }
    }

    /// Runs one scan window. `Ok(None)` means "not seen, scan again".
    async fn find(&mut self) -> core::result::Result<Option<C::Target>, Exit> {
        let find = self.connector.find(self.policy.scan_window);
        match unless_stopped(&mut self.stop, find).await? {
            Ok(Some(target)) => Ok(Some(target)),
            Ok(None) => {
                // No sleep: the empty window already paced us.
                let _ = self.count_failure(Error::NotFound).await?;
                self.emit(Event::WaitingForDevice).await?;
                Ok(None)
            }
            Err(error) => {
                self.failure(1, error).await?;
                Ok(None)
            }
        }
    }

    /// Counts a failed attempt. Ends the stream with [`Event::GaveUp`] when
    /// the budget is exhausted, otherwise hands the error back.
    async fn count_failure(&mut self, error: Error) -> core::result::Result<Error, Exit> {
        self.attempts = self.attempts.saturating_add(1);
        if self
            .policy
            .max_attempts
            .is_some_and(|max| self.attempts >= max)
        {
            // The stream ends either way; a lost final event is acceptable
            // only if the consumer is already gone or stopping.
            let _ = self
                .emit(Event::GaveUp {
                    attempts: self.attempts,
                    last_error: error,
                })
                .await;
            return Err(Exit::GaveUp);
        }
        Ok(error)
    }

    /// Records a failed attempt, emits [`Event::Retrying`] and sleeps the
    /// backoff delay.
    async fn failure(&mut self, retry: u32, error: Error) -> core::result::Result<(), Exit> {
        let error = self.count_failure(error).await?;
        let delay = self.policy.backoff(retry);
        self.emit(Event::Retrying {
            attempt: self.attempts,
            delay,
            error,
        })
        .await?;
        unless_stopped(&mut self.stop, tokio::time::sleep(delay)).await
    }

    /// Sends an event, racing against stop and consumer loss.
    async fn emit(&mut self, event: Event<S::Item>) -> core::result::Result<(), Exit> {
        unless_stopped(&mut self.stop, self.tx.send(event))
            .await?
            .map_err(|_| Exit::ConsumerGone)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::ReconnectPolicy;

    #[test]
    fn backoff_is_bounded_by_the_exponential_cap() {
        let backoff = ReconnectPolicy::default();
        for retry in 1_u32..=8 {
            let cap = Duration::from_secs(1)
                .saturating_mul(2_u32.saturating_pow(retry.saturating_sub(1)))
                .min(Duration::from_secs(15));
            for _ in 0..200 {
                assert!(backoff.backoff(retry) <= cap, "retry {retry}");
            }
        }
        assert!(backoff.backoff(u32::MAX) <= Duration::from_secs(15));
    }

    #[test]
    fn defaults_are_pinned() {
        let policy = ReconnectPolicy::default();
        assert_eq!(policy.scan_window, Duration::from_secs(30));
        assert_eq!(policy.connect_timeout, Duration::from_secs(30));
        assert_eq!(policy.connect_attempts_per_scan, 5);
        assert_eq!(policy.initial_backoff, Duration::from_secs(1));
        assert_eq!(policy.max_backoff, Duration::from_secs(15));
        assert_eq!(policy.max_attempts, None);
    }
}
