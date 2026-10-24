//! Core services shared by every Kadra component.
//!
//! This crate deliberately has no dependency on the UI layer or on Windows
//! APIs so that it can be tested anywhere and used by every other crate.

pub mod config;
pub mod geometry;
pub mod image;
pub mod logging;
pub mod paths;

pub use config::{Config, ConfigError, ImageFormat, VideoCodec};
pub use geometry::{Point, Rect};
pub use image::BgraImage;
pub use paths::Paths;
