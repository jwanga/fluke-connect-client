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
