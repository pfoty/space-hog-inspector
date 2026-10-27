//! Everything that differs between operating systems. Each platform module
//! provides the same functions; the rest of the app reaches them through
//! here.

use std::path::PathBuf;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(not(any(target_os = "macos", windows)))]
compile_error!("Space Hog Inspector supports macOS and Windows only.");

/// What the scanner needs to know about one file or folder.
pub struct FileFacts {
    /// Space it takes up on disk, not its length.
    pub size: u64,
    pub links: u64,
    /// Identifies it across hard links, when known.
    pub id: Option<(u64, u64)>,
    /// The volume it's on, when known.
    pub volume: Option<u64>,
}

/// Free and total space of a volume, where it's mounted, and its file system.
pub struct VolumeInfo {
    pub free: u64,
    pub total: u64,
    pub mount: PathBuf,
    pub fs_type: String,
}

impl VolumeInfo {
    pub fn used_fraction(&self) -> f32 {
        if self.total == 0 { 0.0 } else { 1.0 - self.free as f32 / self.total as f32 }
    }
}
