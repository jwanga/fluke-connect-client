//! A reading stream that survives disconnects.
//!
//! [`ReconnectingReadings`] runs a supervisor task that finds the device,
//! connects, streams readings, and starts over whenever the connection is
//! lost: it re-scans in fixed windows (the device sets the pace by
//! advertising), retries failed connections with exponential backoff, and
//! never gives up unless [`ReconnectPolicy::max_attempts`] says so.
//!
//! Every reconnection goes through a fresh scan on purpose. With btleplug on
//! `CoreBluetooth`, a peripheral handle that has disconnected once is dead:
//! its notification stream stays silent forever and connecting through it
//! fails. Only a handle produced by a new scan works, so
//! [`Connector::find`] is called before every reconnection attempt.
//!
//! The engine is generic over [`Connector`], so the policy can be tested
//! without hardware; the built-in Bluetooth backend provides one through
//! `backend::Adapter::readings_with_reconnect`.

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
use crate::protocol::ReadingNotification;
use crate::transport::Transport;

/// Finds and connects to a Fluke Connect device on behalf of
/// [`ReconnectingReadings`].
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

/// Tunables for [`ReconnectingReadings`].
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
        }
    }
}

/// One step in the life of a reconnecting stream.
#[derive(Debug)]
#[non_exhaustive]
pub enum Event {
    /// Connected and subscribed; readings follow.
    Connected,
    /// A decoded reading.
    Reading(ReadingNotification),
    /// A payload that failed to decode; the connection is kept.
    BadReading(Error),
    /// The connection was lost; a new scan starts.
    Disconnected,
    /// A scan window passed without seeing the device; scanning again.
    WaitingForDevice,
    /// A connect attempt failed; sleeping `delay` before the next one.
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

/// Stops a [`ReconnectingReadings`] from anywhere.
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

/// A reading stream that survives disconnects. See the [module docs](self).
///
/// Dropping it aborts the supervisor task *without* disconnecting the
/// device; prefer [`StopHandle::stop`] followed by draining the stream, which
/// disconnects cleanly.
#[derive(Debug)]
#[must_use = "streams do nothing unless polled"]
pub struct ReconnectingReadings {
    /// Events from the supervisor task.
    rx: mpsc::Receiver<Event>,
    /// The supervisor task; aborted on drop.
    task: JoinHandle<()>,
    /// Stop flag shared with the handles given out by `stop_handle`.
    stop: StopHandle,
}

impl ReconnectingReadings {
    /// Starts supervising.
    ///
    /// `initial`, if given, is connected to before any scan; pass the device
    /// you just discovered.
    ///
    /// # Panics
    ///
    /// Panics if called outside a Tokio runtime.
    pub fn new<C: Connector>(
        connector: C,
        initial: Option<C::Target>,
        policy: ReconnectPolicy,
    ) -> Self {
        let (stop_tx, stop_rx) = watch::channel(false);
        let (tx, rx) = mpsc::channel(EVENT_BUFFER);
        let task = tokio::spawn(async move {
            let mut runner = Runner {
                connector,
                backoff: Backoff {
                    initial: policy.initial_backoff,
                    max: policy.max_backoff,
                },
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

impl Stream for ReconnectingReadings {
    type Item = Event;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Event>> {
        self.get_mut().rx.poll_recv(cx)
    }
}

impl Drop for ReconnectingReadings {
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
    /// The [`ReconnectingReadings`] was dropped.
    ConsumerGone,
    /// [`ReconnectPolicy::max_attempts`] was reached.
    GaveUp,
}

/// What a session step produced.
enum Step {
    /// Stop was requested.
    Stop,
    /// The reading stream yielded an item, or ended (`None`).
    Item(Option<Result<ReadingNotification>>),
}

/// Exponential backoff with full jitter.
#[derive(Debug)]
struct Backoff {
    /// Delay cap for the first retry.
    initial: Duration,
    /// Upper bound on the cap.
    max: Duration,
}

impl Backoff {
    /// Delay for retry number `retry` (1-based): uniform in
    /// `[0, min(initial * 2^(retry - 1), max)]`.
    fn delay(&self, retry: u32) -> Duration {
        let factor = 2_u32.saturating_pow(retry.saturating_sub(1));
        let cap = self.initial.saturating_mul(factor).min(self.max);
        let cap_ms = u64::try_from(cap.as_millis()).unwrap_or(u64::MAX);
        // `RandomState` is randomly seeded per instance, which is all the
        // randomness jitter needs; it avoids a dependency on a rand crate.
        let roll = RandomState::new().build_hasher().finish();
        Duration::from_millis(roll.checked_rem(cap_ms.saturating_add(1)).unwrap_or(0))
    }
}

/// Resolves when stop is requested; pends forever if every [`StopHandle`]
/// is gone, so a dropped handle is never mistaken for a stop.
async fn stopped(rx: &mut watch::Receiver<bool>) {
    if rx.wait_for(|stop| *stop).await.is_err() {
        core::future::pending::<()>().await;
    }
}

/// Disconnects a device on the way out, bounded in time and ignoring errors.
async fn disconnect_quietly<T: Transport>(device: &FlukeDevice<T>) {
    let _ = tokio::time::timeout(STOP_DISCONNECT_TIMEOUT, device.disconnect()).await;
}

/// State owned by the supervisor task.
struct Runner<C: Connector> {
    /// Finds and connects to the device.
    connector: C,
    /// Retry delays.
    backoff: Backoff,
    /// Tunables.
    policy: ReconnectPolicy,
    /// Where events go.
    tx: mpsc::Sender<Event>,
    /// Stop flag.
    stop: watch::Receiver<bool>,
    /// Consecutive failures since the last successful connection.
    attempts: u32,
}

impl<C: Connector> Runner<C> {
    /// The supervisor loop: scan, connect with retries, stream, repeat.
    async fn run(&mut self, mut initial: Option<C::Target>) -> Exit {
        'outer: loop {
            let target = match initial.take() {
                Some(target) => target,
                None => match self.find().await {
                    Ok(Some(target)) => target,
                    Ok(None) => continue 'outer,
                    Err(exit) => return exit,
                },
            };
            for retry in 1..=self.policy.connect_attempts_per_scan.max(1) {
                let connected = tokio::select! {
                    biased;
                    () = stopped(&mut self.stop) => return Exit::Stopped,
                    connected = self.connector.connect(&target, self.policy.connect_timeout) => connected,
                };
                let device = match connected {
                    Ok(device) => device,
                    Err(error) => {
                        if let Err(exit) = self.failure(retry, error).await {
                            return exit;
                        }
                        continue;
                    }
                };
                let subscribed = tokio::select! {
                    biased;
                    () = stopped(&mut self.stop) => {
                        disconnect_quietly(&device).await;
                        return Exit::Stopped;
                    }
                    subscribed = device.readings() => subscribed,
                };
                let mut readings = match subscribed {
                    Ok(readings) => readings,
                    Err(error) => {
                        disconnect_quietly(&device).await;
                        if let Err(exit) = self.failure(retry, error).await {
                            return exit;
                        }
                        continue;
                    }
                };
                self.attempts = 0;
                if let Err(exit) = self.emit(Event::Connected).await {
                    disconnect_quietly(&device).await;
                    return exit;
                }
                loop {
                    let step = tokio::select! {
                        biased;
                        () = stopped(&mut self.stop) => Step::Stop,
                        item = readings.next() => Step::Item(item),
                    };
                    let event = match step {
                        Step::Stop => {
                            disconnect_quietly(&device).await;
                            return Exit::Stopped;
                        }
                        Step::Item(Some(Ok(reading))) => Event::Reading(reading),
                        Step::Item(Some(Err(error))) => Event::BadReading(error),
                        Step::Item(None) => {
                            if let Err(exit) = self.emit(Event::Disconnected).await {
                                return exit;
                            }
                            continue 'outer;
                        }
                    };
                    if let Err(exit) = self.emit(event).await {
                        disconnect_quietly(&device).await;
                        return exit;
                    }
                }
            }
        }
    }

    /// Runs one scan window. `Ok(None)` means "not seen, scan again".
    async fn find(&mut self) -> core::result::Result<Option<C::Target>, Exit> {
        let found = tokio::select! {
            biased;
            () = stopped(&mut self.stop) => return Err(Exit::Stopped),
            found = self.connector.find(self.policy.scan_window) => found,
        };
        match found {
            Ok(Some(target)) => Ok(Some(target)),
            Ok(None) => {
                self.attempts = self.attempts.saturating_add(1);
                if self.gave_up() {
                    let _ = self.tx.try_send(Event::GaveUp {
                        attempts: self.attempts,
                        last_error: Error::NotFound,
                    });
                    return Err(Exit::GaveUp);
                }
                // No sleep: the empty window already paced us.
                self.emit(Event::WaitingForDevice).await?;
                Ok(None)
            }
            Err(error) => {
                self.failure(1, error).await?;
                Ok(None)
            }
        }
    }

    /// Records a failed attempt, emits [`Event::Retrying`] and sleeps the
    /// backoff delay, or ends the stream with [`Event::GaveUp`].
    async fn failure(&mut self, retry: u32, error: Error) -> core::result::Result<(), Exit> {
        self.attempts = self.attempts.saturating_add(1);
        if self.gave_up() {
            let _ = self.tx.try_send(Event::GaveUp {
                attempts: self.attempts,
                last_error: error,
            });
            return Err(Exit::GaveUp);
        }
        let delay = self.backoff.delay(retry);
        self.emit(Event::Retrying {
            attempt: self.attempts,
            delay,
            error,
        })
        .await?;
        tokio::select! {
            biased;
            () = stopped(&mut self.stop) => Err(Exit::Stopped),
            () = tokio::time::sleep(delay) => Ok(()),
        }
    }

    /// Whether the attempt budget is exhausted.
    fn gave_up(&self) -> bool {
        self.policy
            .max_attempts
            .is_some_and(|max| self.attempts >= max)
    }

    /// Sends an event, racing against stop and consumer loss.
    async fn emit(&mut self, event: Event) -> core::result::Result<(), Exit> {
        tokio::select! {
            biased;
            () = stopped(&mut self.stop) => Err(Exit::Stopped),
            sent = self.tx.send(event) => sent.map_err(|_| Exit::ConsumerGone),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Backoff, ReconnectPolicy};

    #[test]
    fn backoff_is_bounded_by_the_exponential_cap() {
        let backoff = Backoff {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(15),
        };
        for retry in 1_u32..=8 {
            let cap = Duration::from_secs(1)
                .saturating_mul(2_u32.saturating_pow(retry.saturating_sub(1)))
                .min(Duration::from_secs(15));
            for _ in 0..200 {
                assert!(backoff.delay(retry) <= cap, "retry {retry}");
            }
        }
        assert!(backoff.delay(u32::MAX) <= Duration::from_secs(15));
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
