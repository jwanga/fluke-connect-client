//! Hardware-in-the-loop tests. Ignored by default; run with
//! `FLUKE_CONNECT_HW=1 cargo test --all-features -- --ignored`.
#![cfg(feature = "ble")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "hardware tests fail loudly and report progress"
)]

use std::time::Duration;

use fluke_connect_client::backend::Adapter;
use fluke_connect_client::reconnect::{Event, ReconnectPolicy};
use futures_util::StreamExt as _;

/// Whether the environment opts in to hardware tests.
fn enabled() -> bool {
    std::env::var_os("FLUKE_CONNECT_HW").is_some_and(|v| v == "1")
}

#[tokio::test]
#[ignore = "needs a powered-on Fluke Connect device in range"]
async fn streams_readings_from_a_real_device() {
    if !enabled() {
        eprintln!("FLUKE_CONNECT_HW is not set; skipping");
        return;
    }
    let adapter = Adapter::open().await.expect("adapter");
    let device = adapter
        .connect_first(Duration::from_secs(90))
        .await
        .expect("a Fluke Connect device must be advertising");

    let info = device.device_info().await.expect("device info");
    assert!(info.model.is_some(), "model number should be readable");
    let battery = device.battery_level().await.expect("battery level");
    assert!(battery <= 100);

    let mut readings = device.readings().await.expect("subscribe");
    for _ in 0..5 {
        let next = tokio::time::timeout(Duration::from_secs(10), readings.next())
            .await
            .expect("a reading within 10 s")
            .expect("stream open");
        let reading = next.expect("decodes");
        eprintln!("{}", reading.primary());
    }
    match device.current_ascii_reading().await {
        Ok(display) => eprintln!("ascii display: {display}"),
        Err(e) => eprintln!("ascii display unavailable: {e}"),
    }
    device.disconnect().await.expect("disconnect");
}

/// Waits for an event matching `want`, skipping other events, within `limit`.
async fn wait_for(
    events: &mut fluke_connect_client::reconnect::ReconnectingReadings,
    limit: Duration,
    want: impl Fn(&Event) -> bool + Send + Sync,
) -> Event {
    tokio::time::timeout(limit, async {
        loop {
            let event = events.next().await.expect("stream still open");
            eprintln!("event: {event:?}");
            if want(&event) {
                return event;
            }
        }
    })
    .await
    .expect("event within the time limit")
}

#[tokio::test]
#[ignore = "needs a Fluke Connect device that will be power-cycled by hand"]
async fn survives_a_device_power_cycle() {
    if std::env::var_os("FLUKE_CONNECT_HW_POWERCYCLE").is_none_or(|v| v != "1") {
        eprintln!("FLUKE_CONNECT_HW_POWERCYCLE is not set; skipping");
        return;
    }
    let adapter = Adapter::open().await.expect("adapter");
    let device = adapter
        .find_first(Duration::from_secs(90))
        .await
        .expect("a Fluke Connect device must be advertising");
    let mut events = adapter.readings_with_reconnect(&device, ReconnectPolicy::default());
    let stop = events.stop_handle();

    wait_for(&mut events, Duration::from_secs(60), |e| {
        matches!(e, Event::Connected)
    })
    .await;
    wait_for(&mut events, Duration::from_secs(30), |e| {
        matches!(e, Event::Reading(_))
    })
    .await;
    eprintln!(
        "POWER-CYCLE THE ADAPTER NOW: hold its button until the LED goes off, then hold it again until it flashes."
    );
    wait_for(&mut events, Duration::from_secs(180), |e| {
        matches!(e, Event::Disconnected)
    })
    .await;
    wait_for(&mut events, Duration::from_secs(300), |e| {
        matches!(e, Event::Connected)
    })
    .await;
    wait_for(&mut events, Duration::from_secs(30), |e| {
        matches!(e, Event::Reading(_))
    })
    .await;

    stop.stop();
    tokio::time::timeout(Duration::from_secs(10), async {
        while events.next().await.is_some() {}
    })
    .await
    .expect("stream drains after stop");
}
