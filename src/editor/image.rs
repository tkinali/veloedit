//! Image document: decoded off-thread, downscaled to a sane preview size.

use std::{fs, path::Path};

use iced::widget::image::Handle;

use crate::detect::ImageFormat;

/// Longest edge kept in the GPU texture; bigger images are downscaled once.
const MAX_PREVIEW_DIM: u32 = 4096;

pub struct ImageDoc {
    pub handle: Handle,
    /// Original pixel size (not the downscaled preview).
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub format: &'static str,
}

impl ImageDoc {
    /// Blocking decode; call from a worker task.
    pub fn load(path: &Path, fmt: ImageFormat) -> Result<Self, String> {
        let bytes = fs::metadata(path).map_err(|e| e.to_string())?.len();
        let img = image::ImageReader::open(path)
            .map_err(|e| e.to_string())?
            .with_guessed_format()
            .map_err(|e| e.to_string())?
            .decode()
            .map_err(|e| e.to_string())?;
        let (width, height) = (img.width(), img.height());
        let img = if width.max(height) > MAX_PREVIEW_DIM {
            img.thumbnail(MAX_PREVIEW_DIM, MAX_PREVIEW_DIM)
        } else {
            img
        };
        let rgba = img.into_rgba8();
        let (w, h) = rgba.dimensions();
        Ok(Self {
            handle: Handle::from_rgba(w, h, rgba.into_raw()),
            width,
            height,
            bytes,
            format: fmt.name(),
        })
    }
}
