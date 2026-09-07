//! Decoding a snapshot into the working image the workbench edits.
//!
//! The working image is orientation-normalized, opaque RGB: alpha is
//! composited onto `#FFFFFF` here, once, so colour analysis, preview and the
//! final composite all see the same pixels. Display metadata that PNG can
//! carry safely (ICC, sRGB/gamma/chromaticity, pixel density) is read here
//! and re-emitted by the exporter; everything else — EXIF, XMP — is dropped.

use std::io::{BufReader, Cursor, Read, Seek};
use std::path::Path;
use std::sync::{Arc, Mutex};

use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};

use crate::error::{AppError, AppResult};

/// Largest image the workbench accepts. 5504×3072 is the biggest output the
/// generators produce; this leaves headroom while keeping the RGBA working
/// buffers (~4 bytes/pixel, several of them) inside a few hundred MB.
pub const MAX_PIXELS: u64 = 40_000_000;

#[derive(Clone, Debug, Default)]
pub struct PngMeta {
    pub pixel_dims: Option<png::PixelDimensions>,
    pub srgb: Option<png::SrgbRenderingIntent>,
    pub gamma: Option<png::ScaledFloat>,
    pub chromaticities: Option<png::SourceChromaticities>,
}

#[derive(Debug)]
pub struct DecodedSource {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGB8, already flattened onto white.
    pub rgb: Vec<u8>,
    pub had_alpha: bool,
    /// Embedded profile, if the container had one.
    pub icc: Option<Vec<u8>>,
    pub png: PngMeta,
    /// Dots per inch from a JPEG JFIF header, when present.
    pub jfif_dpi: Option<(u32, u32)>,
}

impl DecodedSource {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let i = ((y as usize) * (self.width as usize) + x as usize) * 3;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }

    /// Whether the embedded profile describes an RGB device, which is the
    /// only kind an RGB PNG may carry. Anything else (CMYK, Gray, unknown)
    /// must not be written as if it applied to our pixels.
    pub fn icc_is_rgb(&self) -> bool {
        self.icc
            .as_deref()
            .is_some_and(|icc| icc.len() >= 128 && &icc[16..20] == b"RGB ")
    }
}

/// Dimensions after orientation normalization, without keeping the pixels.
pub fn probe_dimensions(bytes: &[u8]) -> AppResult<(u32, u32)> {
    let decoded = decode_bytes(bytes)?;
    Ok((decoded.width, decoded.height))
}

pub fn decode_file(path: &Path) -> AppResult<DecodedSource> {
    let bytes = std::fs::read(path).map_err(|error| {
        AppError::new(
            "workbench_source_unreadable",
            format!("无法读取源图：{error}"),
        )
    })?;
    decode_bytes(&bytes)
}

pub fn decode_bytes(bytes: &[u8]) -> AppResult<DecodedSource> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| AppError::new("workbench_source_undecodable", error.to_string()))?;
    let format = reader.format();
    let mut decoder = reader.into_decoder().map_err(|error| {
        AppError::new(
            "workbench_source_undecodable",
            format!("无法识别图片格式：{error}"),
        )
    })?;
    let (w, h) = decoder.dimensions();
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(AppError::with_detail(
            "workbench_source_too_large",
            "图片像素数超过工作台支持上限",
            serde_json::json!({ "width": w, "height": h, "max_pixels": MAX_PIXELS }),
        ));
    }
    let icc = decoder.icc_profile().ok().flatten();
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).map_err(|error| {
        AppError::new(
            "workbench_source_undecodable",
            format!("图片解码失败：{error}"),
        )
    })?;
    image.apply_orientation(orientation);

    let had_alpha = image.color().has_alpha();
    let (width, height) = (image.width(), image.height());
    let rgb = if had_alpha {
        flatten_onto_white(&image.into_rgba8())
    } else {
        image.into_rgb8().into_raw()
    };

    let png = if format == Some(ImageFormat::Png) {
        read_png_meta(bytes)
    } else {
        PngMeta::default()
    };
    let jfif_dpi = if format == Some(ImageFormat::Jpeg) {
        jfif_density(bytes)
    } else {
        None
    };

    Ok(DecodedSource {
        width,
        height,
        rgb,
        had_alpha,
        icc,
        png,
        jfif_dpi,
    })
}

/// Standard straight-alpha "over" onto opaque white, rounded to nearest.
fn flatten_onto_white(image: &image::RgbaImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(image.width() as usize * image.height() as usize * 3);
    for pixel in image.pixels() {
        let [r, g, b, a] = pixel.0;
        let a = u32::from(a);
        let blend = |c: u8| -> u8 { ((u32::from(c) * a + 255 * (255 - a) + 127) / 255) as u8 };
        out.push(blend(r));
        out.push(blend(g));
        out.push(blend(b));
    }
    out
}

fn read_png_meta(bytes: &[u8]) -> PngMeta {
    let decoder = png::Decoder::new(BufReader::new(Cursor::new(bytes)));
    let Ok(reader) = decoder.read_info() else {
        return PngMeta::default();
    };
    let info = reader.info();
    PngMeta {
        pixel_dims: info.pixel_dims,
        srgb: info.srgb,
        gamma: info.gama_chunk,
        chromaticities: info.chrm_chunk,
    }
}

/// Pixel density from the JFIF APP0 segment, in dots per inch. Densities in
/// dots per centimetre are converted; aspect-only densities (unit 0) are
/// ignored because they carry no absolute scale.
fn jfif_density(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut cursor = Cursor::new(bytes);
    let mut marker = [0u8; 2];
    cursor.read_exact(&mut marker).ok()?;
    if marker != [0xFF, 0xD8] {
        return None;
    }
    loop {
        cursor.read_exact(&mut marker).ok()?;
        if marker[0] != 0xFF {
            return None;
        }
        let kind = marker[1];
        if kind == 0xDA || kind == 0xD9 {
            return None;
        }
        let mut len = [0u8; 2];
        cursor.read_exact(&mut len).ok()?;
        let len = u16::from_be_bytes(len) as usize;
        if len < 2 {
            return None;
        }
        let mut body = vec![0u8; len - 2];
        cursor.read_exact(&mut body).ok()?;
        if kind == 0xE0 && body.len() >= 12 && &body[0..5] == b"JFIF\0" {
            let unit = body[7];
            let x = u32::from(u16::from_be_bytes([body[8], body[9]]));
            let y = u32::from(u16::from_be_bytes([body[10], body[11]]));
            return match unit {
                1 if x > 0 && y > 0 => Some((x, y)),
                2 if x > 0 && y > 0 => Some((
                    (x as f64 * 2.54).round() as u32,
                    (y as f64 * 2.54).round() as u32,
                )),
                _ => None,
            };
        }
        let _ = cursor.stream_position();
    }
}

/// A two-slot cache of decoded sources keyed by asset id.
///
/// Region analysis runs on every selection change, and decoding a 5504×3072
/// JPEG each time would make the preview lag behind the pointer. Two slots
/// cover the common case of comparing two projects; the buffers are large,
/// so this deliberately does not grow.
#[derive(Default)]
pub struct SourceCache {
    slots: Mutex<Vec<(String, Arc<DecodedSource>)>>,
}

impl SourceCache {
    pub fn get_or_decode(&self, asset_id: &str, path: &Path) -> AppResult<Arc<DecodedSource>> {
        if let Some(hit) = self.lookup(asset_id) {
            return Ok(hit);
        }
        let decoded = Arc::new(decode_file(path)?);
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        slots.retain(|(id, _)| id != asset_id);
        if slots.len() >= 2 {
            slots.remove(0);
        }
        slots.push((asset_id.to_string(), Arc::clone(&decoded)));
        Ok(decoded)
    }

    fn lookup(&self, asset_id: &str) -> Option<Arc<DecodedSource>> {
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let index = slots.iter().position(|(id, _)| id == asset_id)?;
        let entry = slots.remove(index);
        let hit = Arc::clone(&entry.1);
        slots.push(entry);
        Some(hit)
    }

    pub fn forget(&self, asset_id: &str) {
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        slots.retain(|(id, _)| id != asset_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_is_flattened_onto_white_without_dark_fringes() {
        let mut image = image::RgbaImage::new(2, 1);
        image.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        image.put_pixel(1, 0, image::Rgba([0, 0, 0, 128]));
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, ImageFormat::Png).unwrap();
        let decoded = decode_bytes(&out.into_inner()).unwrap();
        assert!(decoded.had_alpha);
        assert_eq!(
            decoded.pixel(0, 0),
            [255, 255, 255],
            "全透明像素必须变成纯白"
        );
        assert_eq!(
            decoded.pixel(1, 0),
            [127, 127, 127],
            "半透明黑应与白色正确混合"
        );
    }

    #[test]
    fn png_display_metadata_is_read() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_pixel_dims(Some(png::PixelDimensions {
                xppu: 11811,
                yppu: 11811,
                unit: png::Unit::Meter,
            }));
            encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[1, 2, 3]).unwrap();
        }
        let decoded = decode_bytes(&bytes).unwrap();
        assert_eq!(decoded.png.pixel_dims.map(|d| d.xppu), Some(11811));
        assert_eq!(decoded.png.srgb, Some(png::SrgbRenderingIntent::Perceptual));
        assert!(!decoded.had_alpha);
        assert_eq!(decoded.pixel(0, 0), [1, 2, 3]);
    }

    #[test]
    fn jfif_density_parses_inch_and_centimetre_units() {
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        jpeg.extend_from_slice(b"JFIF\0");
        jpeg.extend_from_slice(&[1, 1, 1, 0x01, 0x2C, 0x01, 0x2C, 0, 0]);
        jpeg.extend_from_slice(&[0xFF, 0xD9]);
        assert_eq!(jfif_density(&jpeg), Some((300, 300)));
        jpeg[13] = 2;
        jpeg[14] = 0;
        jpeg[15] = 118;
        jpeg[16] = 0;
        jpeg[17] = 118;
        assert_eq!(jfif_density(&jpeg), Some((300, 300)));
        assert_eq!(jfif_density(b"\x89PNG"), None);
    }

    #[test]
    fn oversized_images_are_refused_before_decoding() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 20_000, 20_000);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::One);
            let mut writer = encoder.write_header().unwrap();
            writer.write_chunk(png::chunk::IDAT, &[]).unwrap();
            drop(writer);
        }
        // Header only: the decoder reads dimensions before touching pixels.
        let error = decode_bytes(&bytes).unwrap_err();
        assert_eq!(error.code, "workbench_source_too_large");
    }
}
