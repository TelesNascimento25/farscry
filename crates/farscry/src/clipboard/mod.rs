#[cfg(target_os = "macos")]
use anyhow::Result;
#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "macos")]
pub fn check_clipboard_file_path(text: &str) -> Option<PathBuf> {
    let path = PathBuf::from(text.trim());
    if path.exists() && path.is_file() {
        return Some(path);
    }
    None
}

#[cfg(target_os = "macos")]
pub fn supported_image_extension(path: &Path) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "tiff" | "tif" => Ok(()),
        "pdf" => anyhow::bail!("PDF not supported. Export as PNG first."),
        "svg" => anyhow::bail!("SVG not supported. Export as PNG first."),
        other => anyhow::bail!("File type .{other} not supported. Use PNG or JPG."),
    }
}
