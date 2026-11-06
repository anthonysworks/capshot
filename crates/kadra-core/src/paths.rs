//! Well-known locations used by the application.

use std::path::{Path, PathBuf};

use directories::{ProjectDirs, UserDirs};

const APP_NAME: &str = "Kadra";

/// Resolved application directories.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Clone)]
pub struct Paths {
    config_dir: PathBuf,
    log_dir: PathBuf,
    edit_dir: PathBuf,
    pictures_dir: PathBuf,
    videos_dir: PathBuf,
}

impl Paths {
    /// Resolves the directories for the current user.
    ///
    /// Returns `None` only if the user profile cannot be located, which on
    /// Windows means the process runs without a valid user account.
    pub fn resolve() -> Option<Self> {
        let project = ProjectDirs::from("", "", APP_NAME)?;
        let user = UserDirs::new()?;
        let home = user.home_dir();

        let pictures = user
            .picture_dir()
            .map_or_else(|| home.join("Pictures"), Path::to_path_buf);
        let videos = user
            .video_dir()
            .map_or_else(|| home.join("Videos"), Path::to_path_buf);

        Some(Self {
            config_dir: project.config_dir().to_path_buf(),
            log_dir: project.data_local_dir().join("logs"),
            edit_dir: project.data_local_dir().join("edit"),
            pictures_dir: pictures.join(APP_NAME),
            videos_dir: videos.join(APP_NAME),
        })
    }

    /// Builds a set of paths rooted in a single directory (tests, portable mode).
    pub fn rooted_at(root: &Path) -> Self {
        Self {
            config_dir: root.join("config"),
            log_dir: root.join("logs"),
            edit_dir: root.join("edit"),
            pictures_dir: root.join("pictures"),
            videos_dir: root.join("videos"),
        }
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn history_file(&self) -> PathBuf {
        self.config_dir.join("history.toml")
    }

    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }

    /// Editing material kept next to recordings: the video without cursor
    /// and the cursor track.
    pub fn edit_dir(&self) -> &Path {
        &self.edit_dir
    }

    /// Recordings being edited, not saved yet: they reach the destination
    /// folder only when exported (or when the editor closes).
    pub fn drafts_dir(&self) -> PathBuf {
        self.edit_dir.join("drafts")
    }

    /// Default destination for screenshots.
    pub fn pictures_dir(&self) -> &Path {
        &self.pictures_dir
    }

    /// Default destination for recordings.
    pub fn videos_dir(&self) -> &Path {
        &self.videos_dir
    }
}
