//! Connects to the first Fluke Connect device in range and prints readings.
//!
//! ```sh
//! cargo run --example stream
//! ```
#![allow(clippy::print_stdout, clippy::print_stderr, reason = "example program")]

use std::time::Duration;

use fluke_connect_client::backend::Adapter;
use futures_util::StreamExt as _;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let adapter = Adapter::default().await?;
    eprintln!("scanning (hold the adapter's button until its LED flashes)...");
    let device = adapter.connect_first(Duration::from_secs(90)).await?;

    let info = device.device_info().await?;
    eprintln!(
        "connected: {} {} (battery {}%)",
        info.manufacturer.as_deref().unwrap_or("?"),
        info.model.as_deref().unwrap_or("?"),
        device.battery_level().await?
    );

    let mut readings = device.readings().await?;
    while let Some(reading) = readings.next().await {
        match reading {
            Ok(reading) => {
                print!("{}", reading.primary());
                if let Some(secondary) = reading.secondary() {
                    print!("    [{secondary}]");
                }
                println!();
            }
            Err(err) => eprintln!("bad reading: {err}"),
        }
    }
    eprintln!("disconnected");
    Ok(())
}
