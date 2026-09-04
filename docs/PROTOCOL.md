# Fluke Connect Bluetooth Low Energy protocol

This document describes the GATT profile spoken by Fluke Connect devices, as
implemented by this crate. It was assembled from Fluke's own developer guide
for the radio module, several open-source clients, and packet captures from
an **ir3000 FC** infrared adapter attached to a **Fluke 289** multimeter.
Facts verified on that hardware are marked **[verified]**; facts taken from
documentation or other implementations but not observed here are marked
**[documented]**.

## Sources

- Fluke *FBLE Radio Module Developers Guide* V1.0 (2013), filed with the
  FCC under grant T68-FBLE: <https://fccid.io/T68-FBLE/User-Manual/Module-Manual-2101604>.
  The authoritative list of vendor services and characteristics.
- ir3000 FC FCC filing (T68-IRFBLE): test report and internal photos showing
  an MSP430F5659 host MCU driving the FBLE (CC2540) radio module.
- MutekH `libble` Fluke profile (C), which independently recovered the
  binary reading layout: <https://github.com/AntoineMugnier/mutekh/tree/master/libble/profile/fluke>.
- RatioLabs `BLEService` (Android) and `geneva-tools` `3000FC.yaml`, which
  list the same UUIDs.
- A published pc3000 FC serial capture whose `PH=` payloads decode with the
  same layout: <https://randomev.wordpress.com/2017/09/20/fluke-connect-protocol-reverse-engineering/>.

## Device family

Every Fluke Connect wireless product built on the FBLE module exposes the
same profile: the ir3000 FC adapter (Fluke 189, 287, 289 and 789; a
separate variant serves the 1550/1555 insulation testers), the 3000 FC multimeter, the 376 FC and 902 FC clamps, and the
t3000 FC, v3000 FC and a3000 FC modules. The devices differ in which
optional characteristics they populate.

## Advertising **[verified]**

- Local name: `IR 3000 FC` (with spaces). Siblings use names such as
  `376FC`, `902FC` and `3000FC`. Do not rely on the name; filter on the
  reading service UUID instead.
- Advertised service: `b6981800-7562-11e2-b50d-00163e46f8fe`.
- Advertising interval is long (observed roughly 10 s between packets on
  macOS), so scans need to run for tens of seconds.
- The ir3000 FC sleeps until its button is held for more than one second.
  Its LED flashes green while advertising and orange while connected.
- No pairing or bonding is required. ATT MTU is 23 (20-byte payloads).
- The ir3000 FC only advertises once its host meter answers over IR.

## UUIDs

Fluke's base UUID is `B698xxxx-7562-11E2-B50D-00163E46F8FE`. The 16-bit slot
follows the Bluetooth SIG convention: `18xx` services, `29xx` characteristics.

| Slot | Kind | Name | Properties | Status |
|------|------|------|------------|--------|
| 1800 | service | Reading | | **[verified]** |
| 2901 | char | ASCII display (17 B: format byte + 16 ASCII) | read, notify | placeholder with format byte 1 on ir3000 FC **[verified]**; populated on 376 FC / 902 FC **[documented, public captures]**; other family members unconfirmed |
| 290F | char | Binary reading (8 or 16 B) | read, notify | **[verified]** |
| 1801 | service | Connection | | **[verified]** |
| 2902 | char | ID number (u8) | read, write | **[verified]** writes are accepted and read back during the connection, but the value resets to 0 when the connection drops |
| 2903 | char | User string / device name (UTF-8, ≤ 98 B) | read, write | **[verified]** reads `IR 3000 FC` |
| 2904 | char | Force drop | write | **[verified]** write accepted, no disconnect observed within 10 s (twice) |
| 2905 | char | Locator LED (u8: 1 on, 0 off) | write | **[verified]** write accepted |
| 290E | char | POSIX time (u64 LE per the guide) | write, notify | **[verified]** inert on the ir3000 FC: every write of 1 to 20 bytes is rejected with *Invalid Attribute Value Length*, reads are not permitted, and no notification was seen in 40 s of listening |
| 2911 | char | Host firmware update control point | read, write, write-no-rsp, notify | present, **not used by this crate** |
| 2912 | char | Host firmware update buffer | write, write-no-rsp | present, **not used by this crate** |
| 1804 | service | TI OAD (radio firmware) | | present, **not used by this crate** |
| 2913 | char | OAD image identify | write, write-no-rsp, notify | |
| 2914 | char | OAD image block | write, write-no-rsp, notify | |
| 1805 | service | Undocumented | | present on ir3000 FC, absent from the 2013 guide |
| 2918 | char | Unknown | read, write, write-no-rsp, notify | reads `00` |
| 2919 | char | Unknown | notify | |
| 180A | service | Device Information (SIG) | | **[verified]** |
| 2A24 | char | Model number | read | `FLUKE 289` (the attached meter) |
| 2A25 | char | Serial number | read | attached meter's serial, NUL terminated |
| 2A26 | char | Firmware revision | read | `01.00.01` padded with spaces and NUL |
| 2A28 | char | Software revision | read | `V1.41` (the meter's firmware) |
| 2A29 | char | Manufacturer name | read | `Fluke Mfg Co.` |
| 180F | service | Battery (SIG) | | **[verified]** |
| 2A19 | char | Battery level (u8 %) | read, notify | adapter's own batteries |

Firmware update characteristics are deliberately left alone: owners report
bricked adapters after failed updates.

## Binary reading record **[verified]**

The *Binary reading* characteristic notifies whenever the host polls the
meter, about four times per second on the ir3000 FC (mean 0.25 s, up to
1.8 s while the meter auto-ranges). Every notification carries the full
current state; there is no change detection.

The ir3000 FC sends **16 bytes**: two 8-byte records, the primary display
followed by the secondary display. The pc3000 FC dongle and the 3000 FC
multimeter send a single 8-byte record. An all-zero secondary record means
the secondary display is not in use.

Each 8-byte record is two little-endian 32-bit words:

| Word | Bits  | Field | Notes |
|------|-------|-------|-------|
| 0 | 0-20 | mantissa | unsigned, 21 bits; `0x1FFFFF` = no value |
| 0 | 21-24 | state | see below |
| 0 | 25-27 | decimal places | digits after the point |
| 0 | 28-30 | magnitude | SI prefix, see below |
| 0 | 31 | sign | 1 = negative |
| 1 | 0-7 | unit | see below |
| 1 | 8-15 | function | see below |
| 1 | 16-22 | range | 0 on the ir3000 FC / 289 (manual range changes alter decimal places instead) |
| 1 | 23-25 | decade | 0 on the ir3000 FC / 289 |
| 1 | 26-30 | attribute | see below |
| 1 | 31 | capture flag | |

Displayed value = `(sign) mantissa / 10^decimal_places`, in
`magnitude`-prefixed `unit`s.

Worked examples from the capture:

| Bytes | Decoded |
|-------|---------|
| `01 03 00 02 08 22 00 00` | mantissa 769, 1 decimal, state Normal, unit 8 (°F), function 34 (Temperature) → **76.9 °F** |
| `34 00 00 c6 02 0b 00 00` | mantissa 52, negative, 3 decimals, magnitude 4 (milli), unit 2 (V DC), function 11 (mV DC) → **−0.052 mV DC** |
| `ff ff 7f 00 00 0c 00 00` | mantissa `0x1FFFFF`, state 3 (Invalid), function 12 (V DC) → **no reading yet** (just after a dial change) |
| `ff ff 9f 42 0f 2d 00 00` | state 4 (Over range), unit 15 (F), function 45 (Capacitance) → **OL** |
| `ff ff 3f 22 0b 28 00 00` | state 1 (Blank), unit 11 (Ω), function 40 (Resistance) → **blank while auto-ranging** |
| `88 13 00 06 0b 2a 00 00` | mantissa 5000, 3 decimals, unit 11 (Ω), function 42 (Low ohms) → **5.000 Ω** |
| `d5 01 00 04 0b 27 00 08` | mantissa 469, 2 decimals, unit 11 (Ω), function 39 (Continuity), attribute 2 (Short circuit) → **4.69 Ω, beeper on** |

Secondary display example: with the 289 in **V AC LoZ** the notification
was `00 00 00 02 01 07 00 00` + `00 00 00 02 02 07 00 00`, that is
0.0 V AC (function 7, VoltsAcLowZ) on the primary and 0.0 V DC on the
secondary, both tagged with the LoZ function.

### State (4 bits)

| Code | Meaning |
|------|---------|
| 0 | Normal |
| 1 | Blank |
| 2 | Inactive |
| 3 | Invalid |
| 4 | Over range (`OL`) |
| 5 | A/D overload |
| 6 | Open thermocouple |
| 7 | Discharge |
| 8 | Leads |
| 9 | Greater than |
| 10 | Missing phase |
| 11 | Error |
| 12 | Less than |
| 13 | Empty |

### Magnitude (3 bits)

0 none, 1 giga, 2 mega, 3 kilo, 4 milli, 5 micro, 6 nano, 7 pico.

### Unit (8 bits)

0 none, 1 V AC, 2 V DC, 3 A AC, 4 A DC, 5 Hz, 6 %RH, 7 °C, 8 °F, 9 °R,
10 K, 11 Ω, 12 S, 13 duty %, 14 s, 15 F, 16 dB, 17 dBm, 18 W, 19 J, 20 H,
21 psi, 22 mHg, 23 inHg, 24 ftH₂O, 25 mH₂O, 26 inH₂O, 27 inH₂O@60°F,
28 bar, 29 Pa, 30 g/cm², 31 dBV, 32 crest factor, 33 V AC+DC, 34 A AC+DC,
35 %, 36 V/Hz, 37 g, 38 m/s², 39 in/s, 40 mm/s, 41 mil, 42 µm,
43 unknown, 44 TΩ.

### Function (8 bits)

0 none, 1 mV AC, 2 V AC, 3 V AC+DC, 4 mV AC avg, 5 V AC avg, 6 V AC avg+DC,
7 V AC LoZ, 8 mV AC low-pass, 9 V AC low-pass, 10 µV DC, 11 mV DC, 12 V DC,
13 mA AC, 14 A AC, 15 A AC+DC, 16 mA AC avg, 17 A AC avg, 18 A AC avg+DC,
19 µA DC, 20 mA DC, 21 A DC, 22-33 frequency sub-functions, 34 temperature,
35 °F, 36 °C, 37 °R, 38 K, 39 continuity, 40 resistance, 41 conductance,
42 low ohms, 43 phase, 44 A AC inrush, 45 capacitance, 46 diode, 47 V/Hz,
48 mV AC+DC, 49 mA AC+DC, 50 µA AC, 51 µA AC+DC, 52-87 insulation,
installation-tester, pressure and clamp functions. The full table is in
`src/protocol/enums.rs`.

Functions observed on the 289 through the ir3000 FC: 1, 2, 7, 11, 12, 13,
34, 39, 40, 42, 45, 50.

### Attribute (5 bits)

Observed **[verified]**: 2 (short circuit) while the 289 was in continuity
with the leads shorted; 0 otherwise. The rest are **[documented]**.

0 none, 1 open circuit, 2 short circuit, 3 glitch, 4 good diode,
5 negative edge, 6 positive edge, 7 high current, 8 hazardous voltage,
9 low ohms, 10 open glitch, 11 short glitch, 12 peak, 13 sourced,
14 simulated, 15 noise, 16 breakdown.

## ASCII display string **[documented, public captures]**

The characteristic value is **17 bytes**: a format byte followed by 16 ASCII
characters. Format `0x00` is the "meter style" layout below; the developer
guide marks `1` as unassigned. Two public hardware captures from a 376 FC
carry format `0x00`; a third sample, from a 902 FC decoder test, was
re-padded to the same layout.

| Offset | Len | Field      | Contents |
|--------|-----|------------|----------|
| 0      | 6   | reading    | right-justified, space padded: digits, `.`, `-`, or `OL` |
| 6      | 1   | multiplier | `n`, `u`, `m`, `k`, `M`, or a space |
| 7      | 4   | unit       | left-justified, NUL-terminated when shorter than 4: `V`, `A`, `OHMS`, `DEGC`, `DEGF`, `H`, `VHZ`, `F`, `R` |
| 11     | 2   | acdc       | `ac`, `dc`, or spaces |
| 13     | 1   | bolt       | `*` while the hazardous-voltage symbol is lit |
| 14     | 2   | inrush     | `in` while inrush capture is active, else spaces |

Offsets are within the 16-byte text, after the format byte. The guide's `H`
token is hertz (the 3000 FC has no inductance function) and `R` is degrees
Rankine. Expect tokens outside this list on newer firmware; the crate keeps
the raw token when it cannot map one.

Public samples (hex of the full 17-byte value):

| Bytes | Text | Meaning | Source |
|-------|------|---------|--------|
| `00 20 20 20 39 2e 32 20 56 00 20 20 64 63 20 20 20` | `"   9.2 V\0  dc   "` | **9.2 V DC**, Fluke 376 FC | hardware capture in `zach-edf/fluke-app` |
| `00 20 20 20 30 2e 37 20 41 00 20 20 64 63 20 20 20` | `"   0.7 A\0  dc   "` | **0.7 A DC**, Fluke 376 FC | lambdafox.com/clamp-connect |
| `00 20 20 20 30 2e 30 75 46 00 20 20 20 20 20 20 20` | `"   0.0uF\0       "` | **0.0 µF**, Fluke 902 FC | `dwise03-bit/wise2-core` decoder test, re-padded to the documented layout |

Not yet observed on the wire: negative values, `OL`, blank or dashed
displays, `ac`, the bolt and the inrush marker. The crate handles them per
the guide and will need confirming captures.

The ir3000 FC leaves this characteristic at the placeholder
`01 02 03 04 05 00 …` (format byte `1`) and never notifies it **[verified]**.
Its host meter, the 289, is a 50 000-count instrument whose readings need
seven characters, which do not fit the six-character reading field.

## What the adapter talks to

Behind the radio, the ir3000 FC speaks the Fluke 28x infrared serial
protocol (115200 8N1) to the meter, polling it several times a second and
repacking the answer into the binary record above. The meter's `ID`
response (`FLUKE 289,V1.41,<serial>`) is what populates the Device
Information service. The serial protocol is not exposed over Bluetooth.
