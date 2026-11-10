//! Image encoding and file output.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, ImageEncoder};
use capshot_core::{BgraImage, ImageFormat};

/// JPEG quality used for screenshots: visually lossless for UI content.
const JPEG_QUALITY: u8 = 90;

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("cannot encode image: {0}")]
    Image(#[from] image::ImageError),
    #[error("cannot write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("image is empty")]
    Empty,
}

pub type Result<T> = std::result::Result<T, EncodeError>;

pub const fn extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
    }
}

/// Encodes `image` in memory.
///
/// PNG uses fast compression: screenshots compress well anyway and the
/// strongest settings are several times slower for a few percent.
/// WebP is lossless. JPEG has no alpha channel, transparent areas become black.
pub fn encode(image: &BgraImage, format: ImageFormat) -> Result<Vec<u8>> {
    if image.width() == 0 || image.height() == 0 {
        return Err(EncodeError::Empty);
    }
    let (w, h) = (image.width(), image.height());
    let mut out = Vec::with_capacity(image.as_bytes().len() / 4);

    match format {
        ImageFormat::Png => {
            PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Adaptive)
                .write_image(&to_rgba(image), w, h, ExtendedColorType::Rgba8)?;
        }
        ImageFormat::Webp => {
            WebPEncoder::new_lossless(&mut out).write_image(
                &to_rgba(image),
                w,
                h,
                ExtendedColorType::Rgba8,
            )?;
        }
        ImageFormat::Jpeg => {
            JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY).write_image(
                &to_rgb(image),
                w,
                h,
                ExtendedColorType::Rgb8,
            )?;
        }
    }
    Ok(out)
}

fn to_rgba(image: &BgraImage) -> Vec<u8> {
    let mut out = image.as_bytes().to_vec();
    for px in out.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    out
}

fn to_rgb(image: &BgraImage) -> Vec<u8> {
    let src = image.as_bytes().as_chunks::<4>().0;
    let mut out = Vec::with_capacity(src.len() * 3);
    for &[b, g, r, _] in src {
        out.extend_from_slice(&[r, g, b]);
    }
    out
}

/// Current local time, as used in file names.
pub fn now() -> DateTime<Local> {
    Local::now()
}

/// Base file name for a capture taken at `time`, e.g. `Capshot 2026-11-11 19-41-59`.
pub fn file_stem(time: DateTime<Local>) -> String {
    time.format("Capshot %Y-%m-%d %H-%M-%S").to_string()
}

/// First path in `dir` named after `stem` that does not exist yet.
pub fn unique_path(dir: &Path, stem: &str, extension: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{extension}"));
    if !first.exists() {
        return first;
    }
    (2..=u32::MAX)
        .map(|n| dir.join(format!("{stem} ({n}).{extension}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// Saves `image` in `dir`, named after the current local time.
pub fn save_now(image: &BgraImage, format: ImageFormat, dir: &Path) -> Result<PathBuf> {
    save(image, format, dir, Local::now())
}

/// Encodes `image` and writes it to a new file in `dir`.
pub fn save(
    image: &BgraImage,
    format: ImageFormat,
    dir: &Path,
    time: DateTime<Local>,
) -> Result<PathBuf> {
    let bytes = encode(image, format)?;
    let io_err = |path: &Path| {
        let path = path.to_path_buf();
        move |source| EncodeError::Io { path, source }
    };
    fs::create_dir_all(dir).map_err(io_err(dir))?;
    let path = unique_path(dir, &file_stem(time), extension(format));
    // `create_new` never overwrites a file created in the meantime.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(io_err(&path))?;
    io::Write::write_all(&mut file, &bytes).map_err(io_err(&path))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use capshot_core::Point;

    fn sample() -> BgraImage {
        // Blue, green, red, transparent.
        BgraImage::from_raw(
            2,
            2,
            vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 0, 0, 0, 0],
        )
    }

    #[test]
    fn png_round_trip_keeps_pixels_and_alpha() {
        let bytes = encode(&sample(), ImageFormat::Png).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(decoded.get_pixel(0, 0).0, [0, 0, 255, 255]);
        assert_eq!(decoded.get_pixel(0, 1).0, [255, 0, 0, 255]);
        assert_eq!(decoded.get_pixel(1, 1).0[3], 0);
    }

    #[test]
    fn webp_is_lossless() {
        let bytes = encode(&sample(), ImageFormat::Webp).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(decoded.get_pixel(1, 0).0, [0, 255, 0, 255]);
    }

    #[test]
    fn jpeg_encodes() {
        let mut img = BgraImage::new(16, 16);
        assert_eq!(img.pixel(Point::new(0, 0)), Some([0, 0, 0, 255]));
        img.as_bytes_mut().fill(200);
        let bytes = encode(&img, ImageFormat::Jpeg).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
    }

    #[test]
    fn empty_image_is_rejected() {
        assert!(matches!(
            encode(&BgraImage::zeroed(0, 0), ImageFormat::Png),
            Err(EncodeError::Empty)
        ));
    }

    #[test]
    fn file_names_are_unique() {
        let dir = tempfile::tempdir().unwrap();
        let time = Local.with_ymd_and_hms(2026, 11, 11, 9, 5, 3).unwrap();
        let a = save(&sample(), ImageFormat::Png, dir.path(), time).unwrap();
        let b = save(&sample(), ImageFormat::Png, dir.path(), time).unwrap();
        assert_eq!(a.file_name().unwrap(), "Capshot 2026-11-11 09-05-03.png");
        assert_eq!(b.file_name().unwrap(), "Capshot 2026-11-11 09-05-03 (2).png");
    }
}

/// Image format matching a file extension.
pub fn format_of(path: &Path) -> Option<ImageFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "webp" => Some(ImageFormat::Webp),
        _ => None,
    }
}

/// Decodes a PNG, JPEG or WebP file held in memory.
pub fn decode(bytes: &[u8]) -> Result<BgraImage> {
    let rgba = image::load_from_memory(bytes)?.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut data = rgba.into_raw();
    for px in data.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    Ok(BgraImage::from_raw(width, height, data))
}

#[cfg(test)]
mod decode_tests {
    use super::*;

    #[test]
    fn png_round_trip() {
        let image = BgraImage::from_raw(2, 1, vec![1, 2, 3, 255, 10, 20, 30, 128]);
        let png = encode(&image, ImageFormat::Png).unwrap();
        assert_eq!(decode(&png).unwrap(), image);
        assert_eq!(format_of(Path::new("a/b.JPG")), Some(ImageFormat::Jpeg));
        assert_eq!(format_of(Path::new("a.gif")), None);
    }
}
