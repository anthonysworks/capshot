//! Core services shared by every Kadra component.
//!
//! This crate deliberately has no dependency on the UI layer so that the
//! capture and encoding crates can use it without pulling in Tauri.

pub mod config;
pub mod logging;
pub mod paths;

pub use config::{Config, ConfigError, ImageFormat, VideoCodec};
pub use paths::Paths;
