//! The app's logo as files the desktop reads: the PNG itself for Linux, and
//! for Windows an opaque tile of the app's colour with the logo inside it.
//!
//! In the dark theme Windows 11 inverts the colours of a toast's app icon
//! when it judges the icon monochrome: a one-hue mark on transparency, or a
//! dark, mostly grey icon such as a graphite tile with thin violet rings.
//! It left every opaque icon with a solid band of a saturated colour alone,
//! in both themes, so the tile gives every logo that band.
use crate::Error;
use std::{io, path::PathBuf};

/// Largest logo side accepted, in pixels. Toast and notification icons are
/// shown at 16 to 64 points, so a 256 pixel logo is already ample.
const MAX_SIDE: u32 = 1024;

/// The band of background on each side of the logo, as a fraction of the
/// logo's longest side, rounded up: an eighth, so the logo takes four fifths
/// of the tile, and even a logo that fills its whole square is framed by
/// enough colour.
#[cfg(any(target_os = "windows", test))]
const BAND: u32 = 8;

/// A decoded logo: width, height and straight (not premultiplied) RGBA.
/// Elsewhere decoding only validates the PNG.
#[cfg_attr(
    not(any(target_os = "windows", test)),
    expect(dead_code, reason = "only Windows composes the tile")
)]
pub(crate) struct Image {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// Decode a PNG of at most [`MAX_SIDE`] pixels a side.
pub(crate) fn decode(png: &[u8]) -> Result<Image, Error> {
    let mut decoder = png::Decoder::new(io::Cursor::new(png));
    decoder.set_transformations(
        png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
    );
    let mut reader = decoder.read_info().map_err(|_| Error::InvalidInput)?;
    let (width, height) = reader.info().size();
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(Error::InvalidInput);
    }
    let mut buffer = vec![0; reader.output_buffer_size().ok_or(Error::InvalidInput)?];
    let frame = reader
        .next_frame(&mut buffer)
        .map_err(|_| Error::InvalidInput)?;
    let samples = &buffer[..frame.buffer_size()];
    let rgba: Vec<u8> = match frame.color_type {
        png::ColorType::Rgba => samples.to_vec(),
        png::ColorType::GrayscaleAlpha => samples
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[value, alpha]| [value, value, value, alpha])
            .collect(),
        _ => return Err(Error::InvalidInput),
    };
    if rgba.len() != width as usize * height as usize * 4 {
        return Err(Error::InvalidInput);
    }
    Ok(Image {
        width,
        height,
        rgba,
    })
}

/// An opaque square PNG of `background`, with the logo centred at its own
/// pixel size and blended over it. The logo takes about four fifths of the
/// side, so no resampling is needed; the desktop scales the tile to its icon size.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn tile(logo: &Image, background: [u8; 3]) -> Vec<u8> {
    let longest = logo.width.max(logo.height);
    let side = longest + 2 * longest.div_ceil(BAND);
    let (left, top) = ((side - logo.width) / 2, (side - logo.height) / 2);
    let mut rgb = background.repeat(side as usize * side as usize);
    for (index, pixel) in logo.rgba.as_chunks::<4>().0.iter().enumerate() {
        let (x, y) = (index as u32 % logo.width, index as u32 / logo.width);
        let at = ((top + y) as usize * side as usize + (left + x) as usize) * 3;
        let alpha = u32::from(pixel[3]);
        for channel in 0..3 {
            let over = u32::from(pixel[channel]) * alpha;
            let under = u32::from(background[channel]) * (255 - alpha);
            rgb[at + channel] = ((over + under + 127) / 255) as u8;
        }
    }
    encode(side, &rgb)
}

#[cfg(any(target_os = "windows", test))]
fn encode(side: u32, rgb: &[u8]) -> Vec<u8> {
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, side, side);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    // Writing an in-memory image of the size just declared cannot fail.
    if let Ok(mut writer) = encoder.write_header() {
        let _ = writer.write_image_data(rgb);
    }
    png
}

/// Where the app's logo file lives: `notification-logo.png` in a folder
/// named after the app's ID in the per-user cache, `~/.cache` (or
/// `$XDG_CACHE_HOME`) on Linux and `%LOCALAPPDATA%` on Windows. It is
/// rewritten whenever it differs, so deleting it is harmless.
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "windows"))]
pub(crate) fn path(id: &str) -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.cache_dir().join(id).join("notification-logo.png"))
}

/// Write `bytes` to `path` unless it already holds them. The new file is
/// written beside it and renamed over it, so the desktop never reads half
/// an image.
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "windows", test))]
pub(crate) fn write(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    if std::fs::read(path).is_ok_and(|current| current == bytes) {
        return Ok(());
    }
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let partial = path.with_extension("partial");
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, path)
}

/// The file the platform shows for the app's logo, written if needed: the
/// tile on Windows, the PNG itself on Linux. `None` on macOS, which shows
/// the bundle's icon, and when the cache folder cannot be written.
pub(crate) fn file(app: &crate::App) -> Option<PathBuf> {
    let logo = app.logo.as_ref()?;
    #[cfg(target_os = "windows")]
    let bytes = tile(&decode(&logo.png).ok()?, logo.background);
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    let bytes = logo.png.to_vec();
    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "windows"))]
    {
        let path = path(&app.id)?;
        match write(&path, &bytes) {
            Ok(()) => Some(path),
            Err(error) => {
                log::warn!("could not write the notification logo: {error}");
                None
            }
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "windows")))]
    {
        let _ = logo;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(rgba).unwrap();
        writer.finish().unwrap();
        bytes
    }

    #[test]
    fn the_tile_is_opaque_with_the_logo_centred_on_the_background() {
        // 4 by 4: an opaque grey top row, a half-transparent white row, and
        // two transparent rows.
        let mut rgba = Vec::new();
        rgba.extend([60, 60, 60, 255].repeat(4));
        rgba.extend([255, 255, 255, 128].repeat(4));
        rgba.extend([0, 0, 0, 0].repeat(8));
        let logo = decode(&png(4, 4, &rgba)).unwrap();
        let tile = decode(&tile(&logo, [0x6a, 0x4f, 0xd8])).unwrap();
        assert_eq!((tile.width, tile.height), (6, 6));
        let at = |x: usize, y: usize| &tile.rgba[(y * 6 + x) * 4..][..4];
        assert!(tile.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
        // The band: the first row and column are the background.
        assert_eq!(at(0, 0), [0x6a, 0x4f, 0xd8, 255]);
        assert_eq!(at(5, 5), [0x6a, 0x4f, 0xd8, 255]);
        assert_eq!(at(5, 1), [0x6a, 0x4f, 0xd8, 255]);
        assert_eq!(at(1, 1), [60, 60, 60, 255]);
        let blend = |under: u32| ((255 * 128 + under * 127 + 127) / 255) as u8;
        assert_eq!(at(1, 2), [blend(0x6a), blend(0x4f), blend(0xd8), 255]);
        assert_eq!(at(4, 2), [blend(0x6a), blend(0x4f), blend(0xd8), 255]);
        assert_eq!(at(2, 4), [0x6a, 0x4f, 0xd8, 255]);
    }

    #[test]
    fn a_wide_logo_is_centred_in_a_square() {
        let logo = decode(&png(8, 2, &[255; 64])).unwrap();
        let tile = decode(&tile(&logo, [0, 0, 255])).unwrap();
        assert_eq!((tile.width, tile.height), (10, 10));
        let at = |x: usize, y: usize| &tile.rgba[(y * 10 + x) * 4..][..4];
        for x in 1..9 {
            assert_eq!(at(x, 4), [255, 255, 255, 255]);
            assert_eq!(at(x, 5), [255, 255, 255, 255]);
            assert_eq!(at(x, 3), [0, 0, 255, 255]);
            assert_eq!(at(x, 6), [0, 0, 255, 255]);
        }
        assert_eq!(at(0, 4), [0, 0, 255, 255]);
        assert_eq!(at(9, 5), [0, 0, 255, 255]);
    }

    #[test]
    fn only_a_png_of_a_sensible_size_is_a_logo() {
        assert!(decode(b"not a png").is_err());
        assert!(decode(&png(1, 1, &[1, 2, 3, 4])).is_ok());
        assert!(decode(&png(MAX_SIDE + 1, 1, &[0; (MAX_SIDE as usize + 1) * 4])).is_err());
    }

    #[test]
    fn the_logo_is_written_once_and_repaired_when_changed() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("app/notification-logo.png");
        write(&path, b"logo").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"logo");
        let written = std::fs::metadata(&path).unwrap().modified().unwrap();
        write(&path, b"logo").unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            written
        );
        std::fs::write(&path, b"old").unwrap();
        write(&path, b"logo").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"logo");
        assert!(!path.with_extension("partial").exists());
    }
}
