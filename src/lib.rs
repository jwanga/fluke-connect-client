//! Read live measurements from Fluke Connect Bluetooth Low Energy devices.
//!
//! Fluke Connect meters and adapters (the ir3000 FC infrared adapter, the
//! 3000 FC multimeter, the 376 FC and 902 FC clamps, and the t3000 / v3000 /
//! a3000 FC modules) share one vendor GATT profile. This crate decodes that
//! profile and, with the default `ble` feature, connects to devices and
//! streams their readings.
//!
//! # Layers
//!
//! - [`protocol`]: pure, I/O-free parsing of the binary reading record and
//!   the UUID table. Usable with `default-features = false`.
//! - [`transport`]: a small async trait over a connected GATT peripheral so
//!   the client can be driven by any Bluetooth stack or by a test double.
//! - [`client`]: the [`FlukeDevice`] type that subscribes to readings and
//!   exposes the housekeeping characteristics.
//! - `backend`: the built-in [btleplug](https://crates.io/crates/btleplug)
//!   transport, enabled by the `ble` feature.
//!
//! # Hardware note
//!
//! The protocol was verified against an ir3000 FC attached to a Fluke 289.
//! Other family members are expected to work but have not been tested by
//! the authors.
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(test)]
extern crate std;

pub mod protocol;

#[cfg(feature = "std")]
#[cfg_attr(docsrs, doc(cfg(feature = "std")))]
pub mod client;
#[cfg(feature = "std")]
#[cfg_attr(docsrs, doc(cfg(feature = "std")))]
pub mod error;
#[cfg(feature = "std")]
#[cfg_attr(docsrs, doc(cfg(feature = "std")))]
pub mod transport;

#[cfg(feature = "ble")]
#[cfg_attr(docsrs, doc(cfg(feature = "ble")))]
pub mod backend;

#[cfg(feature = "std")]
pub use client::{DeviceInfo, FlukeDevice};
#[cfg(feature = "std")]
pub use error::{Error, Result};

pub use protocol::{
    Attribute, Decade, Function, Magnitude, ProtocolError, Reading, ReadingNotification,
    ReadingState, Unit,
};
