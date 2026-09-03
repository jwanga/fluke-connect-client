//! Wire-format parsing for the Fluke Connect Bluetooth Low Energy protocol.
//!
//! This module performs no I/O and only depends on `core`, so it can be unit
//! tested with byte fixtures and used from `no_std` targets.

pub mod enums;
pub mod error;
pub mod notification;
pub mod reading;
pub mod uuids;

pub use enums::{Attribute, Decade, Function, Magnitude, ReadingState, Unit};
pub use error::ProtocolError;
pub use notification::ReadingNotification;
pub use reading::Reading;
