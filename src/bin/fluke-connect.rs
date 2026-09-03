//! `fluke-connect`: a diagnostic command-line tool for Fluke Connect devices.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a command-line tool's job is to print"
)]

use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand, ValueEnum};
use fluke_connect_client::backend::{Adapter, BtleplugTransport, DiscoveredDevice};
use fluke_connect_client::{FlukeDevice, ReadingNotification};
use futures_util::StreamExt as _;
use tokio::io::AsyncWriteExt as _;

/// Talk to Fluke Connect Bluetooth Low Energy meters and adapters.
#[derive(Debug, Parser)]
#[command(name = "fluke-connect", version, about)]
struct Cli {
    /// Seconds to scan for a device before giving up.
    #[arg(long, global = true, default_value_t = 60)]
    scan_timeout: u64,
    /// Only use the device whose advertised name contains this text.
    #[arg(long, global = true)]
    name: Option<String>,
    /// What to do.
    #[command(subcommand)]
    command: Command,
}

/// Locator LED state.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum Switch {
    /// Turn on.
    On,
    /// Turn off.
    Off,
}

/// Subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Report adapter state and common setup problems.
    Doctor,
    /// List Fluke Connect devices in range.
    Scan,
    /// Show device information, battery, name and ID number.
    Info,
    /// Print live readings.
    Stream {
        /// Emit one JSON object per line instead of text.
        #[arg(long)]
        json: bool,
        /// Stop after this many readings.
        #[arg(long)]
        count: Option<usize>,
        /// Stop after this many seconds.
        #[arg(long)]
        seconds: Option<u64>,
    },
    /// Write raw reading notifications to a JSON Lines file for bug reports.
    Dump {
        /// Output path.
        output: PathBuf,
        /// Stop after this many seconds.
        #[arg(long, default_value_t = 60)]
        seconds: u64,
    },
    /// Turn the device's locator LED on or off.
    Locator {
        /// Desired state.
        state: Switch,
    },
    /// Set the user-assignable device name (up to 98 bytes).
    SetName {
        /// The new name.
        name: String,
    },
    /// Set the ID number shown on devices with a display.
    SetId {
        /// The new ID.
        id: u8,
    },
    /// Set the device clock to the current time.
    SetTime,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let timeout = Duration::from_secs(cli.scan_timeout);

    match cli.command {
        Command::Doctor => doctor().await,
        Command::Scan => {
            let adapter = Adapter::default().await?;
            eprintln!("scanning for {}s...", cli.scan_timeout);
            let devices = adapter.scan(timeout).await?;
            if devices.is_empty() {
                println!("no Fluke Connect devices found (is the device awake and advertising?)");
            }
            for device in devices {
                println!("{device}");
            }
            Ok(())
        }
        Command::Info => {
            let device = connect(timeout, cli.name.as_deref()).await?;
            info(&device).await
        }
        Command::Stream {
            json,
            count,
            seconds,
        } => {
            let device = connect(timeout, cli.name.as_deref()).await?;
            stream(&device, json, count, seconds).await
        }
        Command::Dump { output, seconds } => {
            let device = connect(timeout, cli.name.as_deref()).await?;
            dump(&device, &output, seconds).await
        }
        Command::Locator { state } => {
            let device = connect(timeout, cli.name.as_deref()).await?;
            device.set_locator(matches!(state, Switch::On)).await?;
            println!(
                "locator {}",
                if matches!(state, Switch::On) {
                    "on"
                } else {
                    "off"
                }
            );
            Ok(())
        }
        Command::SetName { name } => {
            let device = connect(timeout, cli.name.as_deref()).await?;
            device.set_name(&name).await?;
            println!("name set to {name:?}");
            Ok(())
        }
        Command::SetId { id } => {
            let device = connect(timeout, cli.name.as_deref()).await?;
            device.set_id_number(id).await?;
            println!("id set to {id}");
            Ok(())
        }
        Command::SetTime => {
            let device = connect(timeout, cli.name.as_deref()).await?;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .context("system clock is before 1970")?
                .as_secs();
            device.set_time(now).await?;
            println!("time set to {now}");
            Ok(())
        }
    }
}

/// Prints adapter diagnostics.
async fn doctor() -> Result<()> {
    match Adapter::default().await {
        Ok(adapter) => {
            println!(
                "adapter: {}",
                adapter.info().await.unwrap_or_else(|e| e.to_string())
            );
            match adapter.is_powered_on().await {
                Ok(true) => println!("state:   powered on"),
                Ok(false) => println!("state:   NOT powered on; turn Bluetooth on"),
                Err(e) => println!("state:   unknown ({e})"),
            }
        }
        Err(e) => println!("adapter: unavailable ({e})"),
    }
    if cfg!(target_os = "macos") {
        println!(
            "note:    on macOS this program uses the Bluetooth permission of the terminal it runs in.\n         If scans find nothing, enable your terminal under System Settings › Privacy & Security › Bluetooth."
        );
    }
    println!(
        "tip:     hold the ir3000 FC button until its LED flashes green; it advertises about every 10 seconds."
    );
    Ok(())
}

/// Scans for a device (optionally by name) and connects.
async fn connect(timeout: Duration, name: Option<&str>) -> Result<FlukeDevice<BtleplugTransport>> {
    let adapter = Adapter::default().await?;
    eprintln!("scanning (up to {}s)...", timeout.as_secs());
    let device: DiscoveredDevice = match name {
        None => adapter.find_first(timeout).await?,
        Some(needle) => adapter
            .scan(timeout)
            .await?
            .into_iter()
            .find(|d| d.name().is_some_and(|n| n.contains(needle)))
            .with_context(|| format!("no device with a name containing {needle:?}"))?,
    };
    eprintln!("connecting to {device}...");
    let connected = adapter.connect(&device).await?;
    eprintln!("connected");
    Ok(connected)
}

/// Prints device information.
async fn info(device: &FlukeDevice<BtleplugTransport>) -> Result<()> {
    let info = device.device_info().await?;
    println!(
        "manufacturer:      {}",
        info.manufacturer.as_deref().unwrap_or("-")
    );
    println!(
        "model:             {}",
        info.model.as_deref().unwrap_or("-")
    );
    println!(
        "serial number:     {}",
        info.serial_number.as_deref().unwrap_or("-")
    );
    println!(
        "firmware revision: {}",
        info.firmware_revision.as_deref().unwrap_or("-")
    );
    println!(
        "software revision: {}",
        info.software_revision.as_deref().unwrap_or("-")
    );
    match device.battery_level().await {
        Ok(level) => println!("battery:           {level}%"),
        Err(e) => println!("battery:           unavailable ({e})"),
    }
    match device.name().await {
        Ok(name) => println!("device name:       {name}"),
        Err(e) => println!("device name:       unavailable ({e})"),
    }
    match device.id_number().await {
        Ok(id) => println!("id number:         {id}"),
        Err(e) => println!("id number:         unavailable ({e})"),
    }
    match device.current_reading().await {
        Ok(reading) => println!("current reading:   {}", reading.primary()),
        Err(e) => println!("current reading:   unavailable ({e})"),
    }
    Ok(())
}

/// Streams readings to stdout.
async fn stream(
    device: &FlukeDevice<BtleplugTransport>,
    json: bool,
    count: Option<usize>,
    seconds: Option<u64>,
) -> Result<()> {
    let mut readings = device.readings().await?;
    let deadline = tokio::time::sleep(seconds.map_or(Duration::MAX, Duration::from_secs));
    tokio::pin!(deadline);
    let mut seen = 0_usize;
    let stdout = std::io::stdout();
    loop {
        tokio::select! {
            () = &mut deadline => break,
            _ = tokio::signal::ctrl_c() => break,
            next = readings.next() => {
                let Some(next) = next else {
                    eprintln!("device disconnected");
                    break;
                };
                let mut out = stdout.lock();
                match next {
                    Ok(reading) if json => writeln!(out, "{}", json_line(&reading))?,
                    Ok(reading) => {
                        write!(out, "{}", reading.primary())?;
                        if let Some(secondary) = reading.secondary() {
                            write!(out, "    [{secondary}]")?;
                        }
                        writeln!(out)?;
                    }
                    Err(e) => eprintln!("bad reading: {e}"),
                }
                seen = seen.saturating_add(1);
                if count.is_some_and(|max| seen >= max) {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// Serialises a reading as one JSON line.
fn json_line(reading: &ReadingNotification) -> String {
    let primary = reading.primary();
    let value = serde_json::json!({
        "primary": {
            "display": primary.to_string(),
            "value": primary.value(),
            "display_value": primary.display_value(),
            "unit": primary.unit(),
            "function": primary.function(),
            "state": primary.state(),
            "attribute": primary.attribute(),
            "magnitude": primary.magnitude(),
            "raw": hex(primary.raw()),
        },
        "secondary": reading.secondary().map(|s| serde_json::json!({
            "display": s.to_string(),
            "value": s.value(),
            "display_value": s.display_value(),
            "unit": s.unit(),
            "function": s.function(),
            "state": s.state(),
            "attribute": s.attribute(),
            "magnitude": s.magnitude(),
            "raw": hex(s.raw()),
        })),
        "timestamp": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64()),
    });
    value.to_string()
}

/// Writes raw notifications as JSON Lines for fixture capture.
async fn dump(
    device: &FlukeDevice<BtleplugTransport>,
    output: &std::path::Path,
    seconds: u64,
) -> Result<()> {
    use fluke_connect_client::protocol::uuids::BINARY_READING;
    use fluke_connect_client::transport::Transport as _;

    let transport = device.transport();
    let mut notifications = transport.notifications().await?;
    transport.subscribe(BINARY_READING).await?;
    let mut file = tokio::fs::File::create(output)
        .await
        .with_context(|| format!("cannot create {}", output.display()))?;
    let deadline = tokio::time::sleep(Duration::from_secs(seconds));
    tokio::pin!(deadline);
    let mut written = 0_usize;
    eprintln!(
        "capturing to {} for {seconds}s (Ctrl-C to stop early)...",
        output.display()
    );
    loop {
        tokio::select! {
            () = &mut deadline => break,
            _ = tokio::signal::ctrl_c() => break,
            next = notifications.next() => {
                let Some(n) = next else { break };
                let ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0.0, |d| d.as_secs_f64());
                let line = serde_json::json!({
                    "t": ts,
                    "characteristic": format!("{:032x}", n.characteristic),
                    "hex": hex(&n.value),
                });
                file.write_all(format!("{line}\n").as_bytes()).await?;
                written = written.saturating_add(1);
            }
        }
    }
    file.flush().await?;
    eprintln!("wrote {written} notifications");
    Ok(())
}

/// Lower-case hex encoding.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(
        String::with_capacity(bytes.len().saturating_mul(2)),
        |mut s, b| {
            use std::fmt::Write as _;
            // Writing to a String cannot fail.
            let _ = write!(s, "{b:02x}");
            s
        },
    )
}
