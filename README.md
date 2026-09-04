# fluke-connect-client

[![crates.io](https://img.shields.io/crates/v/fluke-connect-client.svg)](https://crates.io/crates/fluke-connect-client)
[![docs.rs](https://docs.rs/fluke-connect-client/badge.svg)](https://docs.rs/fluke-connect-client)
[![CI](https://github.com/jwanga/fluke-connect-client/actions/workflows/ci.yml/badge.svg)](https://github.com/jwanga/fluke-connect-client/actions/workflows/ci.yml)

Read live measurements from **Fluke Connect** Bluetooth Low Energy meters
and adapters from Rust.

Fluke Connect devices share one vendor GATT profile. This crate decodes it
and, with the default `ble` feature, discovers devices, connects, and
streams decoded readings as an async `Stream`. `measurements()` subscribes
to both reading characteristics and locks onto the richer binary record as
soon as the device sends it; the binary and ASCII streams are also available
individually. It also exposes the
housekeeping characteristics: device information, battery level, locator
LED, ID number, device name and clock (the clock write is inert on the
ir3000 FC).

## Hardware support

The protocol is shared across the Fluke Connect family, so the crate should
work with:

- **ir3000 FC** infrared adapter (Fluke 189, 287, 289 and 789, plus the
  1550 / 1555 insulation-tester variant)
- **3000 FC** wireless multimeter
- **376 FC** and **902 FC** clamp meters
- **t3000 FC**, **v3000 FC** and **a3000 FC** wireless modules

> **Tested hardware:** the crate has only been verified against an
> **ir3000 FC attached to a Fluke 289**. Reports and packet captures from
> other family members are very welcome; the `fluke-connect dump` command
> exists for exactly that.

Not supported by design: firmware updates (the update characteristics are
present but deliberately left alone) and downloading logged data from the
meter (the ir3000 FC does not expose it).

## Quick start

```toml
[dependencies]
fluke-connect-client = "0.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
futures-util = "0.3"
```

```rust,no_run
use std::time::Duration;

use fluke_connect_client::backend::Adapter;
use futures_util::StreamExt as _;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let adapter = Adapter::open().await?;
    let device = adapter.connect_first(Duration::from_secs(60)).await?;

    let info = device.device_info().await?;
    println!("connected to {:?}", info.model);

    let mut readings = device.measurements().await?;
    while let Some(reading) = readings.next().await {
        let reading = reading?;
        println!("{}", reading.primary());
        if let Some(secondary) = reading.secondary() {
            println!("  secondary: {secondary}");
        }
    }
    Ok(())
}
```

To keep streaming across disconnects, use the reconnecting stream. It
re-scans and reconnects with backoff, and reports state changes as events:

```rust,no_run
use std::time::Duration;

use fluke_connect_client::backend::Adapter;
use fluke_connect_client::reconnect::{Event, ReconnectPolicy};
use futures_util::StreamExt as _;

# #[tokio::main] async fn main() -> Result<(), Box<dyn std::error::Error>> {
let adapter = Adapter::open().await?;
let device = adapter.find_first(Duration::from_secs(60)).await?;
let mut events = adapter.measurements_with_reconnect(&device, ReconnectPolicy::default());
while let Some(event) = events.next().await {
    match event {
        Event::Item(Ok(measurement)) => println!("{}", measurement.primary()),
        Event::Item(Err(e)) => eprintln!("bad reading: {e}"),
        Event::Disconnected => eprintln!("lost the device; reconnecting"),
        other => eprintln!("{other:?}"),
    }
}
# Ok(()) }
```

`events.stop_handle()` returns a cloneable handle whose `stop()` disconnects
cleanly and ends the stream; the `fluke-connect stream --reconnect` command
calls it on Ctrl-C. `Adapter::stream_with_reconnect` accepts any
`reconnect::Source`: the built-in `Measurements`, `Readings`, `AsciiReadings`
and `BatteryUpdates` mirror the `FlukeDevice` streams, and the trait is small
enough to implement for your own subscription. `ReconnectPolicy` also has
an opt-in `idle_timeout`: set it and a link that stays connected but stops
delivering items is dropped and reconnected; it is off by default because a
meter in HOLD is legitimately silent.

Code that does not care which characteristic a value came from can wrap
either in `Measurement`, which exposes what both sources can answer:

```rust
use fluke_connect_client::{AsciiReading, Measurement, Reading, ReadingState, Unit};

let binary = Measurement::from(Reading::from_bytes(&[0x5C, 0x00, 0x00, 0x02, 0x02, 0x0C, 0x00, 0x00])?);
let ascii = Measurement::from(AsciiReading::from_bytes(b"\x00   9.2 V\x00  dc   ")?);
for measurement in [binary, ascii] {
    assert_eq!(measurement.state(), ReadingState::Normal);
    assert_eq!(measurement.unit(), Unit::VoltsDc);
    assert_eq!(measurement.value(), Some(9.2));
    assert_eq!(measurement.to_string(), "9.2 V DC");
}
# Ok::<(), fluke_connect_client::ProtocolError>(())
```

Each reading carries the display state (normal, blank, `OL`, open
thermocouple, ...), the unit, the meter function, the SI prefix and the
value both as displayed and converted to base units:

```rust
use fluke_connect_client::{Reading, ReadingState, Unit};

let reading = Reading::from_bytes(&[0x54, 0x15, 0x00, 0x42, 0x02, 0x0C, 0x06, 0x01])?;
assert_eq!(reading.state(), ReadingState::Normal);
assert_eq!(reading.unit(), Unit::VoltsDc);
assert_eq!(reading.display_value(), Some(546.0)); // 546.0 mV DC
assert_eq!(reading.value(), Some(0.546));         // volts
assert_eq!(reading.to_string(), "546.0 mV DC");
# Ok::<(), fluke_connect_client::ProtocolError>(())
```

Clamp meters such as the 376 FC and 902 FC also publish the display as
text on a second characteristic. `measurements()` falls back to it
automatically; `FlukeDevice::ascii_readings` streams it alone, and
`AsciiReading` decodes it with the same value and unit semantics:

```rust
use fluke_connect_client::{AsciiReading, AsciiState, Unit};

let display = AsciiReading::from_bytes(b"\x00   9.2 V\x00  dc   ")?;
assert_eq!(display.state(), AsciiState::Normal);
assert_eq!(display.unit(), Unit::VoltsDc);
assert_eq!(display.display_value(), Some(9.2));
assert_eq!(display.to_string(), "9.2 V DC");
# Ok::<(), fluke_connect_client::ProtocolError>(())
```

## Command-line tool

A diagnostic CLI ships behind the `cli` feature:

```sh
cargo install fluke-connect-client --features cli

fluke-connect doctor              # adapter state and permission hints
fluke-connect scan                # list Fluke Connect devices in range
fluke-connect info                # device information, battery, name, ID
fluke-connect stream              # live readings, auto-selecting the source; --json for machine output
fluke-connect stream --binary     # the binary record only
fluke-connect stream --ascii      # the ASCII display string only (376 FC, 902 FC and similar)
fluke-connect stream --reconnect  # keep streaming across disconnects; combines with --binary / --ascii
fluke-connect dump readings.jsonl # raw notification capture for bug reports
fluke-connect locator on          # blink the device LED
```

## Platform notes

- **macOS:** a command-line program inherits the Bluetooth permission of the
  terminal it runs in. If scanning finds nothing or you get a permission
  error, enable your terminal under *System Settings › Privacy & Security ›
  Bluetooth*.
- **Linux:** building needs `libdbus-1-dev` and `pkg-config`; running needs
  BlueZ (`bluetoothd`).
- **Windows:** works on Windows 10 and later with no extra setup.
- The ir3000 FC sleeps until its button is held for about a second, and
  again after roughly 20 minutes idle. It advertises slowly, so allow a scan
  of 30 to 60 seconds. The reconnecting stream keeps scanning and picks the
  adapter up again when its button is pressed.

## Features

| Feature | Default | Enables |
|---------|---------|---------|
| `std`   | yes | the async client and transport trait |
| `ble`   | yes | the built-in btleplug transport (macOS, Linux, Windows) |
| `serde` | no  | `Serialize` on readings, `Serialize` / `Deserialize` on enums and `DeviceInfo` |
| `tracing` | no | `tracing` events from the built-in backend |
| `cli`   | no  | the `fluke-connect` binary |

With `default-features = false` the crate is `no_std` and contains only the
protocol parser, suitable for embedded hosts that bring their own BLE stack.

## Bring your own Bluetooth stack

Implement the small [`Transport`](https://docs.rs/fluke-connect-client/latest/fluke_connect_client/transport/trait.Transport.html)
trait over any connected GATT peripheral and hand it to `FlukeDevice::new`.
The `tests/client_mock.rs` file shows a complete in-memory implementation.

## Protocol documentation

The wire format, UUID table and what was verified on real hardware are
described in [`docs/PROTOCOL.md`](docs/PROTOCOL.md).

## Minimum supported Rust version

Rust 1.88 (btleplug 0.13 uses let-chains on macOS and Windows). The MSRV may be raised in minor releases but will always be at
least six months old.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.

Fluke and Fluke Connect are trademarks of Fluke Corporation. This project is
not affiliated with or endorsed by Fluke.
