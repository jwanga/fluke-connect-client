# Changelog

All notable changes to this project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/jwanga/fluke-connect-client/compare/v0.1.0...v0.2.0) - 2026-09-04

### Added

- *(cli)* stream --reconnect takes --idle-timeout and --max-attempts
- *(reconnect)* optional idle timeout for silent links
- *(cli)* stream --reconnect follows --binary and --ascii
- *(reconnect)* [**breaking**] make the reconnecting stream generic over its source
- *(cli)* stream auto-selects the reading source
- *(client)* add measurements(), an auto-selecting reading stream
- *(protocol)* add MeasurementNotification
- *(protocol)* add Measurement over binary and ASCII readings
- *(reconnect)* add a reconnecting reading stream
- *(cli)* show the ASCII display value in info
- *(protocol)* parse the ASCII display characteristic

### Fixed

- *(cli)* reject zero for the reconnect policy flags
- *(protocol)* drop the trailing space from readings with no unit
- *(cli)* keep the dump error message simple
- *(cli)* create the parent directory for dump output

### Other

- *(cli)* say what --binary and --ascii do under --reconnect
- *(reconnect)* pin the idle_timeout default and name both causes of Disconnected
- *(reconnect)* drop the readings-only shim and the Event default
- *(reconnect)* exercise the ReconnectingReadings alias and fix a doc lint
- apply review findings to the measurements stream
- *(protocol)* drop an unused import
- *(protocol)* apply review findings to Measurement
- hedge the clock explanation and quantify the observation window
- *(protocol)* record what the ID and time writes do on the ir3000 FC
- *(reconnect)* apply review findings
- apply review findings to the ASCII display work
- *(protocol)* add continuity, low-ohms and mV-range captures as fixtures

## [0.1.0](https://github.com/jwanga/fluke-connect-client/releases/tag/v0.1.0) - 2026-09-03

### Added

- *(cli)* add fluke-connect diagnostic tool
- add async client, transport trait and btleplug backend
- *(protocol)* decode Fluke Connect binary reading records

### Other

- raise MSRV to 1.88
- apply conventions review findings
- apply code-review findings
- *(protocol)* add property tests for total parsing
- describe the protocol, design and supported hardware
- initialize fluke-connect-client crate
