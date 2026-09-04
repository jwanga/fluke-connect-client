//! Drives the reconnect supervisor with a scripted connector under paused time.

#![cfg(feature = "ble")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::unused_async_trait_impl,
    clippy::wildcard_enum_match_arm,
    clippy::shadow_unrelated,
    reason = "integration tests may fail loudly; the mock implements async trait methods synchronously"
)]

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::hex;
use common::mock::MockTransport;
use fluke_connect_client::Function;
use fluke_connect_client::protocol::uuids;
use fluke_connect_client::reconnect::{
    BatteryUpdates, Connector, Event, Measurements, Readings, ReconnectPolicy, Reconnecting,
    ReconnectingReadings,
};
use fluke_connect_client::transport::TransportError;
use fluke_connect_client::{Error, FlukeDevice};
use futures_util::StreamExt as _;
use tokio::time::Instant;

/// One scripted outcome.
enum Step {
    /// `find` sees the device.
    Found,
    /// `find` runs a full window and sees nothing.
    NotSeen,
    /// `find` fails.
    ScanError,
    /// `connect` fails.
    ConnectError,
    /// `connect` succeeds with this transport.
    Session(MockTransport),
}

/// A recorded connector call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Find(Duration),
    Connect(Duration),
}

/// Connector that replays scripted outcomes and records its calls.
#[derive(Clone)]
struct ScriptedConnector {
    finds: Arc<Mutex<VecDeque<Step>>>,
    connects: Arc<Mutex<VecDeque<Step>>>,
    calls: Arc<Mutex<Vec<(Call, Instant)>>>,
    alive: Arc<()>,
}

impl ScriptedConnector {
    fn new(finds: Vec<Step>, connects: Vec<Step>) -> Self {
        Self {
            finds: Arc::new(Mutex::new(finds.into())),
            connects: Arc::new(Mutex::new(connects.into())),
            calls: Arc::default(),
            alive: Arc::new(()),
        }
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().iter().map(|(c, _)| *c).collect()
    }

    fn call_times(&self) -> Vec<Instant> {
        self.calls.lock().unwrap().iter().map(|(_, t)| *t).collect()
    }
}

impl Connector for ScriptedConnector {
    type Target = ();
    type Transport = MockTransport;

    async fn find(&self, window: Duration) -> fluke_connect_client::Result<Option<()>> {
        self.calls
            .lock()
            .unwrap()
            .push((Call::Find(window), Instant::now()));
        let step = self
            .finds
            .lock()
            .unwrap()
            .pop_front()
            .expect("find script exhausted");
        match step {
            Step::Found => Ok(Some(())),
            Step::NotSeen => {
                tokio::time::sleep(window).await;
                Ok(None)
            }
            Step::ScanError => Err(TransportError::NoAdapter.into()),
            Step::ConnectError | Step::Session(_) => panic!("connect step in find script"),
        }
    }

    async fn connect(
        &self,
        (): &(),
        timeout: Duration,
    ) -> fluke_connect_client::Result<FlukeDevice<MockTransport>> {
        self.calls
            .lock()
            .unwrap()
            .push((Call::Connect(timeout), Instant::now()));
        let step = self
            .connects
            .lock()
            .unwrap()
            .pop_front()
            .expect("connect script exhausted");
        match step {
            Step::ConnectError => Err(TransportError::Timeout.into()),
            Step::Session(transport) => Ok(FlukeDevice::new(transport)),
            Step::Found | Step::NotSeen | Step::ScanError => panic!("find step in connect script"),
        }
    }
}

const TEMPERATURE: &str = "01030002082200000000000000000000";
/// ASCII 9.2 V DC from a 376 FC.
const ASCII_VOLTS: &str = "00202020392e3220560020206463202020";
/// Binary V AC LoZ record with a secondary display.
const LOZ: &str = "00000002010700000000000202070000";

fn policy() -> ReconnectPolicy {
    ReconnectPolicy::default()
}

async fn next<I>(events: &mut Reconnecting<I>) -> Event<I> {
    tokio::time::timeout(Duration::from_secs(3600), events.next())
        .await
        .expect("an event within the test horizon")
        .expect("stream still open")
}

#[tokio::test(start_paused = true)]
async fn connects_to_the_initial_target_without_scanning() {
    let transport = MockTransport::new();
    let connector = ScriptedConnector::new(vec![], vec![Step::Session(transport.clone())]);
    let mut events = Reconnecting::new(connector.clone(), Readings, Some(()), policy());

    assert!(matches!(next(&mut events).await, Event::Connected));
    assert_eq!(
        connector.calls(),
        vec![Call::Connect(Duration::from_secs(30))]
    );
    assert_eq!(
        transport.subscriptions.lock().unwrap().as_slice(),
        &[uuids::BINARY_READING]
    );

    transport.notify(uuids::BINARY_READING, &hex(TEMPERATURE));
    transport.notify(uuids::BINARY_READING, &[1, 2, 3]);
    assert!(
        matches!(next(&mut events).await, Event::Item(Ok(r)) if r.primary().display_value() == Some(76.9))
    );
    assert!(matches!(
        next(&mut events).await,
        Event::Item(Err(Error::Protocol(_)))
    ));
    assert_eq!(
        connector.calls().len(),
        1,
        "a decode error must not reconnect"
    );
}

#[tokio::test(start_paused = true)]
async fn reconnects_by_rescanning_after_a_disconnect() {
    let first = MockTransport::new();
    let second = MockTransport::new();
    let connector = ScriptedConnector::new(
        vec![Step::Found],
        vec![Step::Session(first.clone()), Step::Session(second.clone())],
    );
    let mut events = Reconnecting::new(connector.clone(), Readings, Some(()), policy());

    assert!(matches!(next(&mut events).await, Event::Connected));
    first.drop_link();
    assert!(matches!(next(&mut events).await, Event::Disconnected));
    assert!(matches!(next(&mut events).await, Event::Connected));
    second.notify(uuids::BINARY_READING, &hex(TEMPERATURE));
    assert!(matches!(next(&mut events).await, Event::Item(Ok(_))));
    assert_eq!(
        connector.calls(),
        vec![
            Call::Connect(Duration::from_secs(30)),
            Call::Find(Duration::from_secs(30)),
            Call::Connect(Duration::from_secs(30)),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn backs_off_between_failed_connects() {
    let transport = MockTransport::new();
    let connector = ScriptedConnector::new(
        vec![],
        vec![
            Step::ConnectError,
            Step::ConnectError,
            Step::ConnectError,
            Step::Session(transport),
        ],
    );
    let mut events = Reconnecting::new(connector.clone(), Readings, Some(()), policy());

    let mut delays = Vec::new();
    for expected_attempt in 1..=3_u32 {
        match next(&mut events).await {
            Event::Retrying { attempt, delay, .. } => {
                assert_eq!(attempt, expected_attempt);
                delays.push(delay);
            }
            other => panic!("expected Retrying, got {other:?}"),
        }
    }
    assert!(matches!(next(&mut events).await, Event::Connected));
    assert!(delays[0] <= Duration::from_secs(1));
    assert!(delays[1] <= Duration::from_secs(2));
    assert!(delays[2] <= Duration::from_secs(4));

    let times = connector.call_times();
    assert_eq!(times.len(), 4);
    for (i, delay) in delays.iter().enumerate() {
        assert_eq!(
            times[i + 1] - times[i],
            *delay,
            "gap before connect {}",
            i + 2
        );
    }
}

#[tokio::test(start_paused = true)]
async fn rescans_after_the_per_scan_connect_budget() {
    let transport = MockTransport::new();
    let connector = ScriptedConnector::new(
        vec![Step::Found],
        vec![
            Step::ConnectError,
            Step::ConnectError,
            Step::Session(transport),
        ],
    );
    let mut p = policy();
    p.connect_attempts_per_scan = 2;
    let mut events = Reconnecting::new(connector.clone(), Readings, Some(()), p);

    assert!(matches!(
        next(&mut events).await,
        Event::Retrying { attempt: 1, .. }
    ));
    assert!(matches!(
        next(&mut events).await,
        Event::Retrying { attempt: 2, .. }
    ));
    assert!(matches!(next(&mut events).await, Event::Connected));
    assert_eq!(
        connector.calls(),
        vec![
            Call::Connect(Duration::from_secs(30)),
            Call::Connect(Duration::from_secs(30)),
            Call::Find(Duration::from_secs(30)),
            Call::Connect(Duration::from_secs(30)),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn waits_for_the_device_without_backoff() {
    let transport = MockTransport::new();
    let connector = ScriptedConnector::new(
        vec![Step::NotSeen, Step::NotSeen, Step::Found],
        vec![Step::Session(transport)],
    );
    let start = Instant::now();
    let mut events = Reconnecting::new(connector.clone(), Readings, None, policy());

    assert!(matches!(next(&mut events).await, Event::WaitingForDevice));
    assert!(matches!(next(&mut events).await, Event::WaitingForDevice));
    assert!(matches!(next(&mut events).await, Event::Connected));
    assert_eq!(Instant::now() - start, Duration::from_secs(60));
}

#[tokio::test(start_paused = true)]
async fn scan_errors_are_backed_off() {
    let transport = MockTransport::new();
    let connector = ScriptedConnector::new(
        vec![Step::ScanError, Step::Found],
        vec![Step::Session(transport)],
    );
    let mut events = Reconnecting::new(connector, Readings, None, policy());

    assert!(matches!(
        next(&mut events).await,
        Event::Retrying {
            attempt: 1,
            error: Error::Transport(TransportError::NoAdapter),
            ..
        }
    ));
    assert!(matches!(next(&mut events).await, Event::Connected));
}

#[tokio::test(start_paused = true)]
async fn gives_up_after_max_attempts() {
    let connector = ScriptedConnector::new(vec![], vec![Step::ConnectError, Step::ConnectError]);
    let mut p = policy();
    p.max_attempts = Some(2);
    let mut events = Reconnecting::new(connector, Readings, Some(()), p.clone());
    assert!(matches!(
        next(&mut events).await,
        Event::Retrying { attempt: 1, .. }
    ));
    assert!(matches!(
        next(&mut events).await,
        Event::GaveUp {
            attempts: 2,
            last_error: Error::Transport(TransportError::Timeout)
        }
    ));
    assert!(events.next().await.is_none());

    let connector = ScriptedConnector::new(vec![Step::NotSeen, Step::NotSeen], vec![]);
    let mut events = Reconnecting::new(connector, Readings, None, p);
    assert!(matches!(next(&mut events).await, Event::WaitingForDevice));
    assert!(matches!(
        next(&mut events).await,
        Event::GaveUp {
            attempts: 2,
            last_error: Error::NotFound
        }
    ));
    assert!(events.next().await.is_none());
}

#[tokio::test(start_paused = true)]
async fn stop_during_backoff_ends_the_stream_promptly() {
    let connector = ScriptedConnector::new(vec![], vec![Step::ConnectError]);
    let mut p = policy();
    p.initial_backoff = Duration::from_secs(3600);
    p.max_backoff = Duration::from_secs(3600);
    let start = Instant::now();
    let mut events = Reconnecting::new(connector.clone(), Readings, Some(()), p);
    let stop = events.stop_handle();

    let Event::Retrying { delay, .. } = next(&mut events).await else {
        panic!("expected Retrying");
    };
    stop.stop();
    assert!(events.next().await.is_none());
    assert!(Instant::now() - start < delay);
    assert_eq!(connector.calls().len(), 1);
    assert!(stop.is_stopped());
}

#[tokio::test(start_paused = true)]
async fn stop_during_a_session_disconnects_the_device() {
    let transport = MockTransport::new();
    let connector = ScriptedConnector::new(vec![], vec![Step::Session(transport.clone())]);
    let mut events = Reconnecting::new(connector, Readings, Some(()), policy());
    let stop = events.stop_handle().clone();

    assert!(matches!(next(&mut events).await, Event::Connected));
    stop.stop();
    assert!(events.next().await.is_none());
    assert_eq!(transport.disconnect_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn dropping_the_stream_aborts_the_supervisor_without_disconnecting() {
    let transport = MockTransport::new();
    let connector = ScriptedConnector::new(vec![], vec![Step::Session(transport.clone())]);
    let mut events = Reconnecting::new(connector.clone(), Readings, Some(()), policy());
    assert!(matches!(next(&mut events).await, Event::Connected));
    drop(events);
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        Arc::strong_count(&connector.alive),
        1,
        "supervisor still holds the connector"
    );
    assert_eq!(transport.disconnect_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn subscribe_failure_is_retried_as_a_connect_failure() {
    let bad = MockTransport::new().failing_subscribe();
    let good = MockTransport::new();
    let connector = ScriptedConnector::new(
        vec![Step::Found],
        vec![Step::Session(bad.clone()), Step::Session(good)],
    );
    let mut events = Reconnecting::new(connector.clone(), Readings, Some(()), policy());

    assert!(matches!(
        next(&mut events).await,
        Event::Retrying { attempt: 1, .. }
    ));
    assert!(matches!(next(&mut events).await, Event::Connected));
    assert_eq!(bad.disconnect_count(), 1);
    // A handle that failed after connecting is dead: the next attempt rescans.
    assert_eq!(
        connector.calls(),
        vec![
            Call::Connect(Duration::from_secs(30)),
            Call::Find(Duration::from_secs(30)),
            Call::Connect(Duration::from_secs(30)),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn measurements_source_locks_onto_binary() {
    let first = MockTransport::new();
    let second = MockTransport::new();
    let connector = ScriptedConnector::new(
        vec![Step::Found],
        vec![Step::Session(first.clone()), Step::Session(second.clone())],
    );
    let mut events = Reconnecting::new(connector.clone(), Measurements, Some(()), policy());

    assert!(matches!(next(&mut events).await, Event::Connected));
    assert_eq!(
        first.subscriptions.lock().unwrap().as_slice(),
        &[uuids::BINARY_READING, uuids::ASCII_READING]
    );
    first.notify(uuids::ASCII_READING, &hex(ASCII_VOLTS));
    assert!(matches!(
        next(&mut events).await,
        Event::Item(Ok(m)) if m.primary().as_ascii().is_some()
    ));
    first.notify(uuids::BINARY_READING, &hex(TEMPERATURE));
    assert!(matches!(
        next(&mut events).await,
        Event::Item(Ok(m)) if m.primary().display_value() == Some(76.9)
    ));
    // Locked on: the ASCII notification is dropped, so the next item is the
    // LoZ record, not 9.2 V.
    first.notify(uuids::ASCII_READING, &hex(ASCII_VOLTS));
    first.notify(uuids::BINARY_READING, &hex(LOZ));
    assert!(matches!(
        next(&mut events).await,
        Event::Item(Ok(m)) if m.primary().as_binary().is_some_and(|r| r.function() == Function::VoltsAcLowZ)
    ));

    first.drop_link();
    assert!(matches!(next(&mut events).await, Event::Disconnected));
    assert!(matches!(next(&mut events).await, Event::Connected));
    assert_eq!(
        second.subscriptions.lock().unwrap().as_slice(),
        &[uuids::BINARY_READING, uuids::ASCII_READING]
    );
    assert_eq!(
        connector.calls(),
        vec![
            Call::Connect(Duration::from_secs(30)),
            Call::Find(Duration::from_secs(30)),
            Call::Connect(Duration::from_secs(30)),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn measurements_source_without_characteristics_is_retried() {
    let bare = MockTransport::new()
        .without_characteristic(uuids::BINARY_READING)
        .without_characteristic(uuids::ASCII_READING);
    let good = MockTransport::new();
    let connector = ScriptedConnector::new(
        vec![Step::Found],
        vec![Step::Session(bare.clone()), Step::Session(good)],
    );
    let mut events = Reconnecting::new(connector.clone(), Measurements, Some(()), policy());

    assert!(matches!(
        next(&mut events).await,
        Event::Retrying {
            attempt: 1,
            error: Error::NoReadingCharacteristic,
            ..
        }
    ));
    assert!(matches!(next(&mut events).await, Event::Connected));
    assert_eq!(bare.disconnect_count(), 1);
    assert_eq!(
        connector.calls(),
        vec![
            Call::Connect(Duration::from_secs(30)),
            Call::Find(Duration::from_secs(30)),
            Call::Connect(Duration::from_secs(30)),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn battery_source_yields_plain_levels() {
    let first = MockTransport::new();
    let second = MockTransport::new();
    let connector = ScriptedConnector::new(
        vec![Step::Found],
        vec![Step::Session(first.clone()), Step::Session(second.clone())],
    );
    let mut events = Reconnecting::new(connector, BatteryUpdates, Some(()), policy());

    assert!(matches!(next(&mut events).await, Event::Connected));
    assert_eq!(
        first.subscriptions.lock().unwrap().as_slice(),
        &[uuids::BATTERY_LEVEL]
    );
    first.notify(uuids::BATTERY_LEVEL, &[60]);
    first.notify(uuids::BATTERY_LEVEL, &[59]);
    assert!(matches!(next(&mut events).await, Event::Item(60)));
    assert!(matches!(next(&mut events).await, Event::Item(59)));

    first.drop_link();
    assert!(matches!(next(&mut events).await, Event::Disconnected));
    assert!(matches!(next(&mut events).await, Event::Connected));
    assert_eq!(
        second.subscriptions.lock().unwrap().as_slice(),
        &[uuids::BATTERY_LEVEL]
    );
}
