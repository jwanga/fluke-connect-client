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
│ reconnect::ReconnectingReadings       (feature "ble")       │
├─────────────────────────────────────────────────────────────┤
│ client::FlukeDevice<T: Transport>     (feature "std")       │
│ transport::Transport trait                                  │
├─────────────────────────────────────────────────────────────┤
│ protocol::{Reading, ReadingNotification, AsciiReading, ...}  │
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
instead of decoding garbage.

### transport

An async trait with five operations: `read`, `write`, `subscribe`,
`notifications` and `disconnect`. Characteristics are addressed by `u128`
UUID. The trait uses return-position `impl Future + Send`, so it is not
object safe; the client is generic over it instead. The notification stream
must end when the connection is lost.

### client

`FlukeDevice<T>` maps the GATT profile onto typed methods. `readings()`
subscribes to the binary reading characteristic and yields
`Result<ReadingNotification>` so decode failures are reported without
tearing down the stream; `ascii_readings()` does the same for the ASCII
display characteristic. Both go through one private subscribe-then-filter
helper. Housekeeping methods (`device_info`,
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

`ReconnectingReadings` is a supervisor task that finds the device, connects,
streams readings and starts over when the link drops. It is generic over a
two-method `Connector` trait (`find` for one scan window, `connect` for one
attempt) so the policy is testable with a scripted connector under paused
Tokio time; `Adapter::readings_with_reconnect` supplies the real one.

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
2. `tests/client_mock.rs` drives the client through an in-memory transport.
3. `tests/reconnect.rs` drives the supervisor with a scripted connector
   under paused Tokio time: backoff timings, per-scan connect budget,
   empty windows, give-up, stop and drop semantics.
4. `tests/hardware.rs` is `#[ignore]` and additionally gated on
   `FLUKE_CONNECT_HW=1`; it connects to a real device and checks the first
   readings decode. A second test, gated on `FLUKE_CONNECT_HW_POWERCYCLE=1`,
   expects the reconnecting stream to survive a power cycle of the device.
5. CI builds the protocol layer for `thumbv7em-none-eabihf` to prove it is
   `no_std`, and checks the package file list so no local files ship.

## Public repository hygiene

Edition 2024, MSRV 1.88, dual MIT / Apache-2.0, clippy `pedantic`,
`nursery`, `cargo` plus selected restriction lints with warnings denied in
CI, `cargo-deny`, release-plz with conventional commits, and an explicit
`include` list in `Cargo.toml`.
