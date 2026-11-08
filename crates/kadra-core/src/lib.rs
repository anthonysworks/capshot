//! Core services shared by every Kadra component.
//!
//! This crate deliberately has no dependency on the UI layer or on Windows
//! APIs so that it can be tested anywhere and used by every other crate.

pub mod base64;
pub mod config;
pub mod geometry;
pub mod history;
pub mod image;
pub mod logging;
pub mod paths;

pub use config::{
    AfterCapture, Appearance, Config, ConfigError, GifSettings, ImageFormat, RecordingFormat,
    Theme, VideoCodec,
};
pub use geometry::{Point, Rect};
pub use history::History;
pub use image::BgraImage;
pub use paths::Paths;
