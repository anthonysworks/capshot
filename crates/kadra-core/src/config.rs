//! User configuration, persisted as TOML.
//!
//! Every field has a default and unknown keys are ignored, so configuration
//! files written by older or newer versions keep loading.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::Paths;

/// Lowest and highest frame rates accepted for recordings.
pub const FPS_RANGE: std::ops::RangeInclusive<u32> = 1..=240;
/// GIF delays are in hundredths of a second; browsers slow down anything
/// faster than 50 FPS.
pub const GIF_FPS_RANGE: std::ops::RangeInclusive<u32> = 1..=50;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read or write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid configuration in {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("cannot serialize configuration: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub hotkeys: Hotkeys,
    pub capture: CaptureSettings,
    pub recording: RecordingSettings,
    pub gif: GifSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    /// Opens the region selection overlay for a screenshot.
    pub capture: String,
    /// Opens the region selection overlay for a recording.
    pub record: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            capture: "PrintScreen".into(),
            record: "Shift+PrintScreen".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)]
pub struct CaptureSettings {
    pub output_dir: PathBuf,
    pub format: ImageFormat,
    pub copy_to_clipboard: bool,
    pub save_to_disk: bool,
    /// What happens after a screenshot or a recording.
    pub after_capture: AfterCapture,
    /// Delay in seconds before the capture is taken.
    pub delay_secs: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AfterCapture {
    /// Open the capture in the editor.
    #[default]
    Editor,
    /// Copy it and show a thumbnail; clicking the thumbnail opens the editor.
    Preview,
    /// Copy it, nothing on screen.
    Nothing,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self {
            output_dir: Paths::resolve()
                .map(|p| p.pictures_dir().to_path_buf())
                .unwrap_or_default(),
            format: ImageFormat::Png,
            copy_to_clipboard: true,
            save_to_disk: true,
            after_capture: AfterCapture::Editor,
            delay_secs: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Png,
    Jpeg,
    Webp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingSettings {
    pub output_dir: PathBuf,
    pub fps: u32,
    pub codec: VideoCodec,
    /// Constant quality factor, 0 (best) to 51 (worst).
    pub quality: u8,
    pub capture_cursor: bool,
    pub system_audio: bool,
    pub microphone: bool,
    /// Seconds counted down on screen before recording starts (0 = none).
    pub countdown_secs: u32,
    /// Last format chosen on the toolbar.
    pub format: RecordingFormat,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordingFormat {
    #[default]
    Mp4,
    Gif,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GifSettings {
    pub fps: u32,
    /// Wider recordings are scaled down (0 = keep the size).
    pub max_width: u32,
    /// Palette quality, 1–100.
    pub quality: u8,
    /// Smoother gradients, slightly larger files.
    pub dither: bool,
    /// Loop forever.
    pub repeat: bool,
}

impl Default for GifSettings {
    fn default() -> Self {
        Self {
            fps: 15,
            max_width: 0,
            quality: 90,
            dither: true,
            repeat: true,
        }
    }
}

impl Default for RecordingSettings {
    fn default() -> Self {
        Self {
            output_dir: Paths::resolve()
                .map(|p| p.videos_dir().to_path_buf())
                .unwrap_or_default(),
            fps: 60,
            codec: VideoCodec::H264,
            quality: 23,
            capture_cursor: true,
            system_audio: true,
            microphone: false,
            countdown_secs: 3,
            format: RecordingFormat::Mp4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
}

impl Config {
    /// Checks values that serde cannot validate on its own.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !FPS_RANGE.contains(&self.recording.fps) {
            return Err(ConfigError::Invalid(format!(
                "frame rate must be between {} and {} (got {})",
                FPS_RANGE.start(),
                FPS_RANGE.end(),
                self.recording.fps
            )));
        }
        if !GIF_FPS_RANGE.contains(&self.gif.fps) {
            return Err(ConfigError::Invalid(format!(
                "GIF frame rate must be between {} and {} (got {})",
                GIF_FPS_RANGE.start(),
                GIF_FPS_RANGE.end(),
                self.gif.fps
            )));
        }
        if !(1..=100).contains(&self.gif.quality) {
            return Err(ConfigError::Invalid(format!(
                "GIF quality must be between 1 and 100 (got {})",
                self.gif.quality
            )));
        }
        if self.recording.countdown_secs > 10 {
            return Err(ConfigError::Invalid(format!(
                "countdown must be at most 10 seconds (got {})",
                self.recording.countdown_secs
            )));
        }
        if self.recording.quality > 51 {
            return Err(ConfigError::Invalid(format!(
                "quality must be between 0 and 51 (got {})",
                self.recording.quality
            )));
        }
        if self.hotkeys.capture.trim().is_empty() || self.hotkeys.record.trim().is_empty() {
            return Err(ConfigError::Invalid("hotkeys cannot be empty".into()));
        }
        if self
            .hotkeys
            .capture
            .eq_ignore_ascii_case(&self.hotkeys.record)
        {
            return Err(ConfigError::Invalid(
                "capture and record hotkeys must be different".into(),
            ));
        }
        Ok(())
    }

    /// Loads the configuration file, creating it with defaults if missing.
    ///
    /// A file that cannot be parsed is renamed to `config.toml.bak` and the
    /// defaults are used instead, so a bad edit never prevents start-up.
    pub fn load_or_init(path: &Path) -> Result<Self, ConfigError> {
        match fs::read_to_string(path) {
            Ok(text) => match Self::parse(path, &text) {
                Ok(config) => Ok(config),
                Err(err) => {
                    tracing::warn!("{err}; falling back to defaults");
                    let backup = path.with_extension("toml.bak");
                    fs::rename(path, &backup).map_err(|source| ConfigError::Io {
                        path: backup,
                        source,
                    })?;
                    let config = Self::default();
                    config.save(path)?;
                    Ok(config)
                }
            },
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let config = Self::default();
                config.save(path)?;
                Ok(config)
            }
            Err(source) => Err(ConfigError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    fn parse(path: &Path, text: &str) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Writes the configuration atomically (temporary file + rename).
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        self.validate()?;
        let io_err = |source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(io_err)?;
        }
        let text = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text).map_err(io_err)?;
        fs::rename(&tmp, path).map_err(io_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        (dir, path)
    }

    #[test]
    fn defaults_are_valid() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn gif_limits_are_checked() {
        let mut config = Config::default();
        config.gif.fps = 60;
        assert!(config.validate().is_err());
        config.gif.fps = 50;
        config.gif.quality = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn missing_file_is_created_with_defaults() {
        let (_dir, path) = temp_config();
        let config = Config::load_or_init(&path).unwrap();
        assert_eq!(config, Config::default());
        assert!(path.exists());
    }

    #[test]
    fn round_trip() {
        let (_dir, path) = temp_config();
        let mut config = Config::default();
        config.recording.fps = 144;
        config.recording.codec = VideoCodec::Av1;
        config.capture.format = ImageFormat::Webp;
        config.recording.format = RecordingFormat::Gif;
        config.gif.max_width = 960;
        config.save(&path).unwrap();
        assert_eq!(Config::load_or_init(&path).unwrap(), config);
    }

    #[test]
    fn partial_file_keeps_defaults_for_missing_keys() {
        let (_dir, path) = temp_config();
        fs::write(&path, "[recording]\nfps = 30\nunknown_key = true\n").unwrap();
        let config = Config::load_or_init(&path).unwrap();
        assert_eq!(config.recording.fps, 30);
        assert_eq!(config.hotkeys, Hotkeys::default());
    }

    #[test]
    fn broken_file_is_backed_up() {
        let (_dir, path) = temp_config();
        fs::write(&path, "this is not toml = = =").unwrap();
        let config = Config::load_or_init(&path).unwrap();
        assert_eq!(config, Config::default());
        assert!(path.with_extension("toml.bak").exists());
    }

    #[test]
    fn out_of_range_fps_is_rejected() {
        let mut config = Config::default();
        config.recording.fps = 0;
        assert!(config.validate().is_err());
        config.recording.fps = 241;
        assert!(config.validate().is_err());
        config.recording.fps = 240;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn identical_hotkeys_are_rejected() {
        let mut config = Config::default();
        config.hotkeys.record = config.hotkeys.capture.to_lowercase();
        assert!(config.validate().is_err());
    }
}
