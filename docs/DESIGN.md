# Design

## Goals

- Let Rust applications consume live readings from any Fluke Connect BLE
  device with a few lines of code, on macOS, Linux and Windows.
- Keep the wire protocol usable without Bluetooth or `std`, so it can be
  unit tested with byte fixtures and reused on embedded hosts.
- Make the Bluetooth stack replaceable and mockable.
- Ship a public crate that meets current ecosystem hygiene expectations.

## Non-goals

- Firmware updates of the radio or host MCU.
- The Fluke 28x infrared serial protocol itself (see `f289ctrl` for that).
- Cloud features of the Fluke Connect app.

## Layers

```
┌─────────────────────────────────────────────────────────────┐
│ fluke-connect (CLI, feature "cli")   examples/stream.rs     │
├─────────────────────────────────────────────────────────────┤
│ backend::Adapter / BtleplugTransport  (feature "ble")       │
│ reconnect::Reconnecting<I> + Source   (feature "ble")       │
├─────────────────────────────────────────────────────────────┤
│ client::FlukeDevice<T: Transport>     (feature "std")       │
│ transport::Transport trait                                  │
├─────────────────────────────────────────────────────────────┤
│ protocol::{Reading, AsciiReading, Measurement, ...}         │
│ (core only, no_std)                                         │
└─────────────────────────────────────────────────────────────┘
```

### protocol

Pure functions over byte slices. `Reading::from_array` cannot fail: every
bit pattern decodes, with unrecognised codes kept in `Unknown(u8)` variants
so newer devices degrade gracefully. `ReadingNotification` accepts the
8-byte single-display form and the 16-byte primary + secondary form. The
UUID table is expressed as `u128` constants so the module has no
dependencies. `AsciiReading` parses the text display characteristic used by
the clamp meters; `from_array` is total over ASCII input, and the 17-byte
framed form is gated on its format byte so unknown layouts fail loudly
instead of decoding garbage. `Measurement` is a closed `Copy` enum over
`Reading` and `AsciiReading` exposing only what both can answer: value,
display value, unit, magnitude, state and `Display`. Its accessors
delegate, so parity is structural rather than re-implemented; its only own
semantics are folding `AsciiState` onto `ReadingState` (dashes to blank,
other text to invalid) and reporting an all-zero binary record as empty.
Source-specific detail stays reachable through `as_binary` and `as_ascii`.
`MeasurementNotification` pairs a primary and optional secondary
`Measurement` (an ASCII source never has one), is built from either
notification type with `From`, and is the item type of the auto-selecting
stream.

### transport

An async trait with five operations: `read`, `write`, `subscribe`,
`notifications` and `disconnect`. Characteristics are addressed by `u128`
UUID. The trait uses return-position `impl Future + Send`, so it is not
object safe; the client is generic over it instead. The notification stream
must end when the connection is lost.

### client

`FlukeDevice<T>` maps the GATT profile onto typed methods. `measurements()`
opens the notification stream, subscribes to the binary and then the ASCII
reading characteristic tolerating only "characteristic not found" (both
missing is `NoReadingCharacteristic`), and yields
`Result<MeasurementNotification>`. Its selection policy is a pure function
driven by `filter_map`: the stream locks onto the binary record when its
first notification arrives, decodable or not, and drops ASCII afterwards.
`readings()` and `ascii_readings()` pin one source, yield
`Result<ReadingNotification>` and `Result<AsciiReading>`, and share one
private subscribe-then-filter helper; decode failures are reported as items
without tearing down any of these streams. Housekeeping methods (`device_info`,
`battery_level`, `set_locator`, `set_name`, `set_id_number`, `set_time`,
`force_drop`) are thin, documented wrappers.

### backend

`Adapter` wraps a btleplug adapter: `scan` filters advertisements on the
Fluke reading service UUID (and post-filters, because BlueZ merges scan
filters across clients), `connect` establishes the GATT connection,
discovers services and builds a UUID to characteristic map.
`BtleplugTransport` implements `Transport`; its notification stream is the
btleplug notification stream merged with the adapter's disconnect events so
it terminates on disconnect. No btleplug or tokio types appear in the public
API, so the backend can change or gain siblings without a breaking release.

### reconnect

`Reconnecting<I>` is a supervisor task that finds the device, connects,
opens a `Source` on it and forwards the source's items as `Event::Item`,
starting over when the link drops. It is generic over two small traits:
`Connector` (`find` for one scan window, `connect` for one attempt) supplies
fresh connections, and `Source` (`open` on a connected `FlukeDevice`) says
what to subscribe to. The stream is parameterised on the item type, not the
source, and the supervisor owns the source value so every connection gets a
fresh subscription. `Readings`,
`Measurements`, `AsciiReadings` and `BatteryUpdates` are zero-sized sources
delegating to the client; a source that fails to open (for example
`Measurements` on a device with neither reading characteristic) is treated
as a failed connect: disconnect, back off, re-scan. Both traits are testable
with a scripted connector and an in-memory transport under paused Tokio
time; `Adapter::stream_with_reconnect` supplies the real connector, with
`measurements_with_reconnect` as the shorthand for the recommended source.

Every reconnection goes through a fresh scan on purpose: after a
disconnect, btleplug on CoreBluetooth keeps a stale peripheral whose
notification stream is silent forever, so the connector forgets cached
peripherals and re-finds the device by address. Scan windows repeat at a
fixed length because the device sets the pace by advertising; empty
windows repeat immediately, while scan errors and connect failures back
off (1 s doubling to 15 s with full jitter, then back to scanning). The adapter's event broadcast is shallow, so it is subscribed
per attempt and polled continuously.

Events flow through a bounded channel; `StopHandle::stop` disconnects the
current device and ends the stream, while dropping the stream aborts the
task without disconnecting. Known limits: a stop that lands inside a
connect cannot cancel the OS-side attempt, and there is no liveness
watchdog for a link that stays up but goes silent.

## Error handling

`Error` is `#[non_exhaustive]` and wraps `ProtocolError` and
`TransportError`. `TransportError::PermissionDenied` carries the macOS
terminal-permission hint in its message because that is the most common
first-run failure.

## Testing

1. Protocol unit tests with byte fixtures from real captures and from
   published pc3000 FC logs, plus round-trip tests over every enum code.
2. `tests/client_mock.rs` drives the client through an in-memory transport,
   including the four `measurements()` cases: binary only, ASCII only, both
   (binary wins after its first notification), and neither (error).
3. `tests/reconnect.rs` drives the supervisor with a scripted connector
   under paused Tokio time: backoff timings, per-scan connect budget,
   empty windows, give-up, stop and drop semantics, and per source that
   `Measurements` opens both characteristics on every connection and
   retries a device that has neither, and that `BatteryUpdates` yields
   bare levels.
4. `tests/hardware.rs` is `#[ignore]` and additionally gated on
   `FLUKE_CONNECT_HW=1`; it connects to a real device and checks the first
   readings decode. A second test, gated on `FLUKE_CONNECT_HW_POWERCYCLE=1`,
   expects the reconnecting stream to survive a power cycle of the device.
5. `tests/measurement_parity.rs` pairs binary records with the ASCII text
   of the same display and requires equal value, unit, state and `Display`
   through `Measurement`; property tests check that wrapping a
   reading changes nothing except the documented empty-slot rule.
6. CI builds the protocol layer for `thumbv7em-none-eabihf` to prove it is
   `no_std`, and checks the package file list so no local files ship.

## Public repository hygiene

Edition 2024, MSRV 1.88, dual MIT / Apache-2.0, clippy `pedantic`,
`nursery`, `cargo` plus selected restriction lints with warnings denied in
CI, `cargo-deny`, release-plz with conventional commits, and an explicit
`include` list in `Cargo.toml`.
