use std::path::Path;

use anyhow::Result;
use capture_protocol::WindowCandidate;

use crate::source::CaptureSource;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub fn enumerate_windows(thumbnail_directory: Option<&Path>) -> Result<Vec<WindowCandidate>> {
    linux::enumerate_windows(thumbnail_directory)
}

#[cfg(target_os = "windows")]
pub fn enumerate_windows(thumbnail_directory: Option<&Path>) -> Result<Vec<WindowCandidate>> {
    windows::enumerate_windows(thumbnail_directory)
}

#[cfg(target_os = "macos")]
pub fn enumerate_windows(thumbnail_directory: Option<&Path>) -> Result<Vec<WindowCandidate>> {
    macos::enumerate_windows(thumbnail_directory)
}

#[cfg(target_os = "linux")]
pub fn open_window_source(kind: &str, value: &str) -> Result<Box<dyn CaptureSource>> {
    linux::open_window_source(kind, value)
}

#[cfg(target_os = "windows")]
pub fn open_window_source(kind: &str, value: &str) -> Result<Box<dyn CaptureSource>> {
    windows::open_window_source(kind, value)
}

#[cfg(target_os = "macos")]
pub fn open_window_source(kind: &str, value: &str) -> Result<Box<dyn CaptureSource>> {
    macos::open_window_source(kind, value)
}

pub(crate) fn save_thumbnail(rgba: &[u8], width: u32, height: u32, path: &Path) -> Result<()> {
    let image = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| anyhow::anyhow!("invalid RGBA thumbnail buffer"))?;
    let thumbnail = image::imageops::thumbnail(&image, 480, 300);
    thumbnail.save_with_format(path, image::ImageFormat::Png)?;
    Ok(())
}
