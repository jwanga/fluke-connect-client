//! `fluke-connect`: a diagnostic command-line tool for Fluke Connect devices.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a command-line tool's job is to print"
)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand, ValueEnum};
use fluke_connect_client::backend::{Adapter, BtleplugTransport, DiscoveredDevice};
use fluke_connect_client::protocol::uuids::{ASCII_READING, BINARY_READING};
use fluke_connect_client::reconnect::{Event, ReconnectPolicy};
use fluke_connect_client::transport::{BoxStream, Transport as _};
use fluke_connect_client::{AsciiReading, FlukeDevice, Reading, ReadingNotification};
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
    #[arg(long = "name", global = true)]
    device_name: Option<String>,
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
    /// Commands that connect to a device first.
    #[command(flatten)]
    Device(DeviceCommand),
}

/// Subcommands that operate on a connected device.
#[derive(Debug, Subcommand)]
enum DeviceCommand {
    /// Show device information, battery, name, ID number and GATT table.
    Info,
    /// Print live readings (the binary record by default).
    Stream {
        /// Emit one JSON object per line instead of text.
        #[arg(long)]
        json: bool,
        /// Stream the ASCII display string (376 FC, 902 FC and similar)
        /// instead of the binary record. Silent on an ir3000 FC.
        #[arg(long)]
        ascii: bool,
        /// Re-scan and reconnect whenever the connection drops.
        #[arg(long, conflicts_with = "ascii")]
        reconnect: bool,
        /// Stop after this many readings.
        #[arg(long)]
        count: Option<usize>,
        /// Stop after this many seconds.
        #[arg(long)]
        seconds: Option<u64>,
    },
    /// Write raw reading and ASCII display notifications to a JSON Lines file for bug reports.
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
        new_name: String,
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
    // RUST_LOG overrides; by default hide btleplug's chatty internal
    // errors (it logs one on every clean CoreBluetooth disconnect).
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,btleplug=off"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let timeout = Duration::from_secs(cli.scan_timeout);

    let command = match cli.command {
        Command::Doctor => return doctor().await,
        Command::Scan => return scan(timeout).await,
        Command::Device(command) => command,
    };

    if let DeviceCommand::Stream {
        json,
        count,
        seconds,
        reconnect: true,
        ..
    } = command
    {
        let (adapter, device) = discover(timeout, cli.device_name.as_deref()).await?;
        return stream_reconnect(&adapter, &device, json, count, seconds).await;
    }

    let device = connect(timeout, cli.device_name.as_deref()).await?;
    let result = run(&device, command).await;
    // Always release the link: BlueZ keeps LE connections alive after the
    // client exits, which would stop the adapter advertising.
    if let Err(e) = device.disconnect().await {
        eprintln!("warning: disconnect failed: {e}");
    }
    result
}

/// Runs a command that needs a connected device.
async fn run(device: &FlukeDevice<BtleplugTransport>, command: DeviceCommand) -> Result<()> {
    match command {
        DeviceCommand::Info => info(device).await,
        DeviceCommand::Stream {
            json,
            ascii,
            count,
            seconds,
            reconnect: _,
        } => stream(device, json, ascii, count, seconds).await,
        DeviceCommand::Dump { output, seconds } => dump(device, &output, seconds).await,
        DeviceCommand::Locator { state } => {
            let on = matches!(state, Switch::On);
            device.set_locator(on).await?;
            println!("locator {}", if on { "on" } else { "off" });
            Ok(())
        }
        DeviceCommand::SetName { new_name } => {
            device.set_name(&new_name).await?;
            println!("name set to {new_name:?}");
            Ok(())
        }
        DeviceCommand::SetId { id } => {
            device.set_id_number(id).await?;
            println!("id set to {id}");
            Ok(())
        }
        DeviceCommand::SetTime => {
            let now = unix_now()?.as_secs();
            device.set_time(now).await?;
            println!("time set to {now}");
            Ok(())
        }
    }
}

/// Prints adapter diagnostics.
async fn doctor() -> Result<()> {
    match Adapter::open().await {
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

/// Lists devices in range.
async fn scan(timeout: Duration) -> Result<()> {
    let adapter = Adapter::open().await?;
    eprintln!("scanning for {}s...", timeout.as_secs());
    let devices = adapter.scan(timeout).await?;
    if devices.is_empty() {
        println!("no Fluke Connect devices found (is the device awake and advertising?)");
    }
    for device in devices {
        println!("{device}");
    }
    Ok(())
}

/// Scans for a device, optionally by name, without connecting.
async fn discover(timeout: Duration, name: Option<&str>) -> Result<(Adapter, DiscoveredDevice)> {
    let adapter = Adapter::open().await?;
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
    Ok((adapter, device))
}

/// Scans for a device (optionally by name) and connects.
async fn connect(timeout: Duration, name: Option<&str>) -> Result<FlukeDevice<BtleplugTransport>> {
    let (adapter, device) = discover(timeout, name).await?;
    eprintln!("connecting to {device}...");
    let connected = adapter.connect(&device).await?;
    eprintln!("connected");
    Ok(connected)
}

/// Streams readings across disconnects, reporting state changes on stderr.
async fn stream_reconnect(
    adapter: &Adapter,
    device: &DiscoveredDevice,
    json: bool,
    count: Option<usize>,
    seconds: Option<u64>,
) -> Result<()> {
    eprintln!("connecting to {device} (will reconnect on loss)...");
    let mut events = adapter.readings_with_reconnect(device, ReconnectPolicy::default());
    let stop = events.stop_handle();
    let deadline = tokio::time::sleep(seconds.map_or(Duration::MAX, Duration::from_secs));
    tokio::pin!(deadline);
    let mut seen = 0_usize;
    let stdout = std::io::stdout();
    loop {
        tokio::select! {
            () = &mut deadline, if !stop.is_stopped() => stop.stop(),
            _ = tokio::signal::ctrl_c(), if !stop.is_stopped() => stop.stop(),
            event = events.next() => {
                let Some(event) = event else { break };
                #[allow(
                    clippy::wildcard_enum_match_arm,
                    reason = "Event is non-exhaustive; unknown future variants are informational"
                )]
                match event {
                    Event::Connected => eprintln!("connected"),
                    // Readings buffered before a stop are drained but not printed,
                    // so --count never overshoots.
                    Event::Reading(reading) if !stop.is_stopped() => {
                        let line = if json { json_line(&reading) } else { text_line(&reading) };
                        writeln!(stdout.lock(), "{line}")?;
                        seen = seen.saturating_add(1);
                        if count.is_some_and(|max| seen >= max) {
                            stop.stop();
                        }
                    }
                    Event::Reading(_) => {}
                    Event::BadReading(e) => eprintln!("bad reading: {e}"),
                    Event::Disconnected => eprintln!("disconnected; scanning again..."),
                    Event::WaitingForDevice => {
                        eprintln!("waiting for device (hold its button until the LED flashes)...");
                    }
                    Event::Retrying { attempt, delay, error } => {
                        eprintln!("connect failed ({error}); retry {attempt} in {:.1}s", delay.as_secs_f64());
                    }
                    Event::GaveUp { attempts, last_error } => {
                        anyhow::bail!("gave up after {attempts} attempts: {last_error}");
                    }
                    other => eprintln!("{other:?}"),
                }
            }
        }
    }
    Ok(())
}

/// Prints device information.
async fn info(device: &FlukeDevice<BtleplugTransport>) -> Result<()> {
    let info = device.device_info().await?;
    let fields = [
        ("manufacturer", &info.manufacturer),
        ("model", &info.model),
        ("serial number", &info.serial_number),
        ("firmware revision", &info.firmware_revision),
        ("software revision", &info.software_revision),
    ];
    for (label, value) in fields {
        println!("{label:<18} {}", value.as_deref().unwrap_or("-"));
    }
    print_or_error(
        "battery",
        device.battery_level().await.map(|b| format!("{b}%")),
    );
    print_or_error("device name", device.name().await);
    print_or_error(
        "id number",
        device.id_number().await.map(|id| id.to_string()),
    );
    print_or_error(
        "current reading",
        device
            .current_reading()
            .await
            .map(|r| r.primary().to_string()),
    );
    print_or_error(
        "ascii display",
        device.current_ascii_reading().await.map(|r| r.to_string()),
    );
    println!("characteristics:");
    let mut uuids: Vec<u128> = device.transport().characteristic_uuids().collect();
    uuids.sort_unstable();
    for uuid in uuids {
        println!("  {}", uuid::Uuid::from_u128(uuid));
    }
    Ok(())
}

/// Prints a labelled value or the error that prevented reading it.
fn print_or_error<E: std::fmt::Display>(label: &str, value: Result<String, E>) {
    match value {
        Ok(v) => println!("{label:<18} {v}"),
        Err(e) => println!("{label:<18} unavailable ({e})"),
    }
}

/// Streams readings to stdout as pre-formatted lines.
async fn stream(
    device: &FlukeDevice<BtleplugTransport>,
    json: bool,
    ascii: bool,
    count: Option<usize>,
    seconds: Option<u64>,
) -> Result<()> {
    let mut lines: BoxStream<'static, fluke_connect_client::Result<String>> = if ascii {
        device
            .ascii_readings()
            .await?
            .map(move |r| {
                r.map(|a| {
                    if json {
                        ascii_json_line(&a)
                    } else {
                        a.to_string()
                    }
                })
            })
            .boxed()
    } else {
        device
            .readings()
            .await?
            .map(move |r| r.map(|n| if json { json_line(&n) } else { text_line(&n) }))
            .boxed()
    };
    let deadline = tokio::time::sleep(seconds.map_or(Duration::MAX, Duration::from_secs));
    tokio::pin!(deadline);
    let mut seen = 0_usize;
    let stdout = std::io::stdout();
    loop {
        tokio::select! {
            () = &mut deadline => break,
            _ = tokio::signal::ctrl_c() => break,
            next = lines.next() => {
                let Some(next) = next else {
                    eprintln!("device disconnected");
                    break;
                };
                match next {
                    Ok(line) => writeln!(stdout.lock(), "{line}")?,
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

/// Formats a binary notification as `primary    [secondary]`.
fn text_line(reading: &ReadingNotification) -> String {
    reading.secondary().map_or_else(
        || reading.primary().to_string(),
        |secondary| format!("{}    [{secondary}]", reading.primary()),
    )
}

/// Serialises an ASCII display value as one JSON line.
fn ascii_json_line(reading: &AsciiReading) -> String {
    serde_json::json!({
        "ascii": {
            "display": reading.to_string(),
            "reading_text": reading.reading_text(),
            "value": reading.value(),
            "display_value": reading.display_value(),
            "state": reading.state(),
            "unit": reading.unit(),
            "unit_token": reading.unit_token(),
            "magnitude": reading.magnitude(),
            "acdc": reading.acdc(),
            "hazardous_voltage": reading.hazardous_voltage(),
            "inrush": reading.inrush(),
            "raw": hex(reading.raw()),
        },
        "timestamp": timestamp(),
    })
    .to_string()
}

/// Serialises a notification as one JSON line.
fn json_line(reading: &ReadingNotification) -> String {
    serde_json::json!({
        "primary": reading_json(reading.primary()),
        "secondary": reading.secondary().map(reading_json),
        "timestamp": timestamp(),
    })
    .to_string()
}

/// Serialises one reading.
fn reading_json(reading: &Reading) -> serde_json::Value {
    serde_json::json!({
        "display": reading.to_string(),
        "value": reading.value(),
        "display_value": reading.display_value(),
        "unit": reading.unit(),
        "function": reading.function(),
        "state": reading.state(),
        "attribute": reading.attribute(),
        "magnitude": reading.magnitude(),
        "raw": hex(reading.raw()),
    })
}

/// Writes raw notifications as JSON Lines for fixture capture.
async fn dump(device: &FlukeDevice<BtleplugTransport>, output: &Path, seconds: u64) -> Result<()> {
    let transport = device.transport();
    let mut notifications = transport.notifications().await?;
    transport.subscribe(BINARY_READING).await?;
    // Best effort: devices other than the ir3000 FC populate the ASCII
    // display, and captures of it are what other-meter owners can send us.
    if let Err(e) = transport.subscribe(ASCII_READING).await {
        eprintln!("note: ASCII display not subscribed: {e}");
    }
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("cannot create directory {}", parent.display()))?;
    }
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
                let line = serde_json::json!({
                    "t": timestamp(),
                    "characteristic": uuid::Uuid::from_u128(n.characteristic).to_string(),
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

/// Seconds since the UNIX epoch as a float, `0.0` if the clock is unusable.
fn timestamp() -> f64 {
    unix_now().map_or(0.0, |d| d.as_secs_f64())
}

/// Time since the UNIX epoch.
fn unix_now() -> Result<Duration> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before 1970")
}

/// Lower-case hex encoding.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(
        String::with_capacity(bytes.len().saturating_mul(2)),
        |mut s, b| {
            // Writing to a String cannot fail.
            let _ = write!(s, "{b:02x}");
            s
        },
    )
}
