//! Authoritative compositing and PNG export.
//!
//! Everything here works in source pixels: the decoded snapshot is the
//! canvas, repair fills and text are painted onto it in document order, then
//! the integer crop is cut out and flips permute pixels. No step resamples
//! the source. The preview canvas never feeds into this; it only shows what
//! this will produce.

use std::path::Path;

use serde::Serialize;
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Transform};

use crate::error::{AppError, AppResult};

use super::doc::{parse_hex_color, Layer, ProjectDoc, RepairGroup, Shape, TextLayer};
use super::geom::PixelRect;
use super::source::DecodedSource;
use super::text::{Fonts, TextSpec};

/// Opaque RGB8 pixels of the final image.
pub struct Composite {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    /// Families that were substituted by the bundled fallback.
    pub missing_fonts: Vec<String>,
}

/// What the export dialog shows before anything is written.
#[derive(Clone, Debug, Serialize)]
pub struct ExportPreview {
    pub width: u32,
    pub height: u32,
    pub missing_fonts: Vec<String>,
    /// Set when export is blocked because the source profile is not safe for the RGB output.
    pub color_note: Option<String>,
    pub had_alpha: bool,
}

pub fn preview(
    source: &DecodedSource,
    doc: &ProjectDoc,
    fonts: &mut Fonts,
) -> AppResult<ExportPreview> {
    check_source(source, doc)?;
    let mut missing = Vec::new();
    for text in all_texts(doc) {
        let layout = fonts.measure(&text_spec(text))?;
        if layout.missing_font && !missing.contains(&text.font.family) {
            missing.push(text.font.family.clone());
        }
    }
    Ok(ExportPreview {
        width: doc.viewport.crop.width as u32,
        height: doc.viewport.crop.height as u32,
        missing_fonts: missing,
        color_note: color_note(source),
        had_alpha: source.had_alpha,
    })
}

fn color_note(source: &DecodedSource) -> Option<String> {
    match &source.icc {
        Some(_) if source.icc_is_rgb() => None,
        Some(_) => Some("源图包含无法安全转换的非 RGB 色彩配置，当前版本无法导出".to_string()),
        None => None,
    }
}

fn check_color_profile(source: &DecodedSource) -> AppResult<()> {
    if source.icc.is_some() && !source.icc_is_rgb() {
        return Err(AppError::new(
            "workbench_color_profile_unsupported",
            "源图包含无法安全转换的非 RGB 色彩配置，已阻止导出",
        ));
    }
    Ok(())
}

fn check_source(source: &DecodedSource, doc: &ProjectDoc) -> AppResult<()> {
    if source.width != doc.source.width || source.height != doc.source.height {
        return Err(AppError::with_detail(
            "workbench_source_mismatch",
            "源图尺寸与工程记录不一致",
            serde_json::json!({
                "source": [source.width, source.height],
                "document": [doc.source.width, doc.source.height]
            }),
        ));
    }
    doc.validate()
}

fn all_texts(doc: &ProjectDoc) -> Vec<&TextLayer> {
    let mut out = Vec::new();
    for layer in &doc.layers {
        match layer {
            Layer::Repair(group) if group.visible => {
                out.extend(group.children.iter().filter(|t| t.visible))
            }
            Layer::Text(text) if text.visible => out.push(text),
            _ => {}
        }
    }
    out
}

pub fn text_spec(text: &TextLayer) -> TextSpec {
    TextSpec {
        text: text.text.clone(),
        width: text.rect.width as f64,
        height: text.rect.height as f64,
        family: text.font.family.clone(),
        size: text.font.size,
        weight: text.font.weight,
        italic: text.font.italic,
        line_height: text.line_height,
        letter_spacing: text.letter_spacing,
        align: text.align.clone(),
        valign: text.valign.clone(),
        auto_fit: text.auto_fit,
    }
}

/// Export phases reported to the UI. `percent` is cumulative over the whole
/// export: compositing owns 0–60, PNG encoding 60–95, the atomic write the rest.
#[derive(Clone, Debug, Serialize)]
pub struct ExportProgress {
    /// `compose` | `encode` | `write` | `done`
    pub stage: &'static str,
    pub percent: u8,
}

pub type ProgressSink<'a> = &'a mut dyn FnMut(ExportProgress);

fn report(progress: &mut ProgressSink<'_>, stage: &'static str, percent: f64) {
    progress(ExportProgress {
        stage,
        percent: percent.round().clamp(0.0, 100.0) as u8,
    });
}

/// Paint the document over the source and apply the viewport.
#[cfg(test)]
pub fn compose(
    source: &DecodedSource,
    doc: &ProjectDoc,
    fonts: &mut Fonts,
) -> AppResult<Composite> {
    compose_with_progress(source, doc, fonts, &mut |_| {})
}

pub fn compose_with_progress(
    source: &DecodedSource,
    doc: &ProjectDoc,
    fonts: &mut Fonts,
    mut progress: ProgressSink<'_>,
) -> AppResult<Composite> {
    check_source(source, doc)?;
    check_color_profile(source)?;
    report(&mut progress, "compose", 0.0);
    let (width, height) = (source.width, source.height);
    let mut pixmap = Pixmap::new(width, height)
        .ok_or_else(|| AppError::new("workbench_compose_failed", "合成缓冲区创建失败"))?;
    {
        let data = pixmap.data_mut();
        for (dst, src) in data.chunks_exact_mut(4).zip(source.rgb.chunks_exact(3)) {
            dst[0] = src[0];
            dst[1] = src[1];
            dst[2] = src[2];
            dst[3] = 255;
        }
    }
    report(&mut progress, "compose", 5.0);

    let mut missing_fonts = Vec::new();
    let total = doc.layers.len().max(1) as f64;
    for (index, layer) in doc.layers.iter().enumerate() {
        match layer {
            Layer::Repair(group) if group.visible => {
                fill_group(&mut pixmap, group)?;
                for text in group.children.iter().filter(|t| t.visible) {
                    draw_text(&mut pixmap, text, fonts, &mut missing_fonts)?;
                }
            }
            Layer::Text(text) if text.visible => {
                draw_text(&mut pixmap, text, fonts, &mut missing_fonts)?
            }
            _ => {}
        }
        report(
            &mut progress,
            "compose",
            5.0 + 50.0 * (index as f64 + 1.0) / total,
        );
    }

    let crop = doc.viewport.crop;
    let mut rgb = crop_rgb(pixmap.data(), width, &crop);
    if doc.viewport.flip_x {
        flip_x(&mut rgb, crop.width as usize, crop.height as usize);
    }
    if doc.viewport.flip_y {
        flip_y(&mut rgb, crop.width as usize, crop.height as usize);
    }
    report(&mut progress, "compose", 60.0);
    Ok(Composite {
        width: crop.width as u32,
        height: crop.height as u32,
        rgb,
        missing_fonts,
    })
}

fn fill_group(pixmap: &mut Pixmap, group: &RepairGroup) -> AppResult<()> {
    let [r, g, b] = parse_hex_color(&group.fill.current)
        .ok_or_else(|| AppError::new("project_color_invalid", "填充色格式无效"))?;
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, 255);
    paint.anti_alias = matches!(group.shape, Shape::Ellipse);
    let rect = group.rect;
    let skia_rect = Rect::from_xywh(
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
    )
    .ok_or_else(|| AppError::new("project_layer_out_of_bounds", "修补区域无效"))?;
    match group.shape {
        Shape::Rect => {
            pixmap.fill_rect(skia_rect, &paint, Transform::identity(), None);
        }
        Shape::Ellipse => {
            let path = PathBuilder::from_oval(skia_rect)
                .ok_or_else(|| AppError::new("project_layer_out_of_bounds", "椭圆区域无效"))?;
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
    Ok(())
}

fn draw_text(
    pixmap: &mut Pixmap,
    text: &TextLayer,
    fonts: &mut Fonts,
    missing: &mut Vec<String>,
) -> AppResult<()> {
    if text.text.trim().is_empty() {
        return Ok(());
    }
    let color = parse_hex_color(&text.color)
        .ok_or_else(|| AppError::new("project_color_invalid", "文字颜色格式无效"))?;
    let (glyphs, layout, dx, dy) = fonts.rasterize(&text_spec(text), color)?;
    if layout.missing_font && !missing.contains(&text.font.family) {
        missing.push(text.font.family.clone());
    }
    let x = text.rect.x as f32 + dx as f32;
    let y = text.rect.y as f32 + dy as f32;
    let transform = if text.angle.abs() > 0.01 {
        let cx = text.rect.x as f32 + text.rect.width as f32 / 2.0;
        let cy = text.rect.y as f32 + text.rect.height as f32 / 2.0;
        Transform::from_rotate_at(text.angle as f32, cx, cy)
    } else {
        Transform::identity()
    };
    let mut paint = PixmapPaint::default();
    paint.quality = if text.angle.abs() > 0.01 {
        tiny_skia::FilterQuality::Bilinear
    } else {
        tiny_skia::FilterQuality::Nearest
    };
    pixmap.draw_pixmap(x as i32, y as i32, glyphs.as_ref(), &paint, transform, None);
    Ok(())
}

fn crop_rgb(rgba: &[u8], width: u32, crop: &PixelRect) -> Vec<u8> {
    let mut out = Vec::with_capacity((crop.width * crop.height * 3) as usize);
    let stride = width as usize * 4;
    for y in crop.y..crop.bottom() {
        let row = &rgba[y as usize * stride..];
        for x in crop.x..crop.right() {
            let i = x as usize * 4;
            out.extend_from_slice(&row[i..i + 3]);
        }
    }
    out
}

fn flip_x(rgb: &mut [u8], width: usize, height: usize) {
    for y in 0..height {
        let row = &mut rgb[y * width * 3..(y + 1) * width * 3];
        for x in 0..width / 2 {
            let (a, b) = (x * 3, (width - 1 - x) * 3);
            for c in 0..3 {
                row.swap(a + c, b + c);
            }
        }
    }
}

fn flip_y(rgb: &mut [u8], width: usize, height: usize) {
    let stride = width * 3;
    for y in 0..height / 2 {
        let (top, bottom) = rgb.split_at_mut((height - 1 - y) * stride);
        top[y * stride..(y + 1) * stride].swap_with_slice(&mut bottom[..stride]);
    }
}

/// Encode `composite` as an opaque RGB PNG with only whitelisted metadata,
/// written through a sibling temp file so `target` is never half a file.
#[cfg(test)]
pub fn write_png(composite: &Composite, source: &DecodedSource, target: &Path) -> AppResult<u64> {
    write_png_with_progress(composite, source, target, &mut |_| {})
}

pub fn write_png_with_progress(
    composite: &Composite,
    source: &DecodedSource,
    target: &Path,
    mut progress: ProgressSink<'_>,
) -> AppResult<u64> {
    use std::io::Write;

    check_color_profile(source)?;
    let png_error = |error: png::EncodingError| {
        AppError::new("workbench_png_failed", format!("PNG 编码失败：{error}"))
    };
    let io_error = |error: std::io::Error| {
        AppError::new("workbench_png_failed", format!("PNG 编码失败：{error}"))
    };
    report(&mut progress, "encode", 60.0);
    let mut bytes = Vec::new();
    {
        let mut info = png::Info::with_size(composite.width, composite.height);
        info.color_type = png::ColorType::Rgb;
        info.bit_depth = png::BitDepth::Eight;
        apply_metadata(&mut info, source);
        let mut encoder = png::Encoder::with_info(&mut bytes, info).map_err(png_error)?;
        encoder.set_compression(png::Compression::Balanced);
        encoder.set_filter(png::Filter::Adaptive);
        let mut writer = encoder.write_header().map_err(png_error)?;
        // Rows go through the streaming writer in bands so a 5504×3072 export
        // can report progress instead of freezing at "encoding".
        let mut stream = writer.stream_writer().map_err(png_error)?;
        let stride = composite.width as usize * 3;
        let rows = composite.height as usize;
        let band = 64usize;
        for start in (0..rows).step_by(band) {
            let end = (start + band).min(rows);
            stream
                .write_all(&composite.rgb[start * stride..end * stride])
                .map_err(io_error)?;
            report(
                &mut progress,
                "encode",
                60.0 + 35.0 * end as f64 / rows.max(1) as f64,
            );
        }
        stream.finish().map_err(png_error)?;
    }
    report(&mut progress, "write", 96.0);
    super::asset::write_atomic(target, &bytes).map_err(|error| {
        AppError::new(
            "workbench_export_write_failed",
            format!("无法写入导出文件：{}", error.message),
        )
    })?;
    report(&mut progress, "done", 100.0);
    Ok(bytes.len() as u64)
}

/// The metadata whitelist: a validated RGB ICC profile, or the source PNG's
/// sRGB/gamma/chromaticity when no ICC profile is present, plus pixel density.
/// Nothing else survives — `Info::with_size` starts empty, so text, EXIF and
/// unknown chunks from the source are simply never copied.
fn apply_metadata(info: &mut png::Info<'static>, source: &DecodedSource) {
    if let Some(icc) = source.icc.as_ref().filter(|_| source.icc_is_rgb()) {
        info.icc_profile = Some(std::borrow::Cow::Owned(icc.clone()));
    } else if source.icc.is_none() && source.png.srgb.is_some() {
        info.srgb = source.png.srgb;
    } else {
        info.source_gamma = source.png.gamma;
        info.source_chromaticities = source.png.chromaticities;
    }
    info.pixel_dims = source.png.pixel_dims.or_else(|| {
        source.jfif_dpi.map(|(x, y)| png::PixelDimensions {
            xppu: (f64::from(x) / 0.0254).round() as u32,
            yppu: (f64::from(y) / 0.0254).round() as u32,
            unit: png::Unit::Meter,
        })
    });
}

#[cfg(test)]
mod tests {
    use super::super::doc::{
        sample_text, Fill, FillSource, OcrState, RepairKind, SourceRef, Viewport, SCHEMA_VERSION,
    };
    use super::super::source::{decode_bytes, PngMeta};
    use super::*;

    fn source(width: u32, height: u32) -> DecodedSource {
        let mut rgb = Vec::new();
        for y in 0..height {
            for x in 0..width {
                rgb.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, 77]);
            }
        }
        DecodedSource {
            width,
            height,
            rgb,
            had_alpha: false,
            icc: None,
            png: PngMeta::default(),
            jfif_dpi: None,
        }
    }

    fn doc(width: u32, height: u32) -> ProjectDoc {
        ProjectDoc {
            schema_version: SCHEMA_VERSION,
            id: "p".into(),
            revision: 1,
            name: "n".into(),
            source: SourceRef {
                asset_id: "a".into(),
                filename: "f.png".into(),
                width,
                height,
                orientation_normalized: true,
                working_color_space: "srgb".into(),
            },
            viewport: Viewport {
                crop: PixelRect::new(0, 0, i64::from(width), i64::from(height)),
                flip_x: false,
                flip_y: false,
            },
            layers: Vec::new(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn group(id: &str, shape: Shape, rect: PixelRect, color: &str) -> Layer {
        Layer::Repair(RepairGroup {
            kind: RepairKind::Repair,
            id: id.into(),
            visible: true,
            shape,
            rect,
            fill: Fill {
                auto: None,
                current: color.into(),
                source: FillSource::Manual,
                uneven: false,
                coverage: None,
            },
            ocr: OcrState::default(),
            children: Vec::new(),
        })
    }

    fn px(c: &Composite, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * c.width + x) * 3) as usize;
        [c.rgb[i], c.rgb[i + 1], c.rgb[i + 2]]
    }

    #[test]
    fn export_progress_is_monotonic_and_ends_at_100() {
        let src = source(64, 48);
        let mut d = doc(64, 48);
        d.layers.push(group(
            "g",
            Shape::Rect,
            PixelRect::new(4, 4, 20, 10),
            "#ff0000",
        ));
        let mut fonts = Fonts::for_tests();
        let mut seen: Vec<(&'static str, u8)> = Vec::new();
        let dir = super::super::asset::tests::temp_dir("progress");
        let target = dir.join("out.png");
        let composite = compose_with_progress(&src, &d, &mut fonts, &mut |p| {
            seen.push((p.stage, p.percent))
        })
        .unwrap();
        write_png_with_progress(&composite, &src, &target, &mut |p| {
            seen.push((p.stage, p.percent))
        })
        .unwrap();
        assert_eq!(seen.first().map(|s| s.1), Some(0));
        assert_eq!(seen.last().copied(), Some(("done", 100)));
        assert!(seen.windows(2).all(|w| w[0].1 <= w[1].1), "{seen:?}");
        assert!(seen.iter().any(|s| s.0 == "encode"));
        assert!(seen.iter().any(|s| s.0 == "write"));
        let decoded = image::open(&target).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (64, 48));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crop_is_exact_and_untouched_pixels_survive() {
        let src = source(300, 200);
        let mut d = doc(300, 200);
        d.viewport.crop = PixelRect::new(17, 23, 101, 59);
        let mut fonts = Fonts::for_tests();
        let out = compose(&src, &d, &mut fonts).unwrap();
        assert_eq!((out.width, out.height), (101, 59));
        assert_eq!(px(&out, 0, 0), src.pixel(17, 23));
        assert_eq!(px(&out, 100, 58), src.pixel(117, 81));
    }

    #[test]
    fn flips_permute_pixels_without_resampling() {
        let src = source(64, 32);
        let mut d = doc(64, 32);
        d.viewport.flip_x = true;
        let mut fonts = Fonts::for_tests();
        let out = compose(&src, &d, &mut fonts).unwrap();
        assert_eq!(px(&out, 0, 0), src.pixel(63, 0));
        assert_eq!(px(&out, 63, 31), src.pixel(0, 31));
        d.flip_y_for_test();
        let out = compose(&src, &d, &mut fonts).unwrap();
        assert_eq!(px(&out, 0, 0), src.pixel(63, 31));
        let mut sorted_src = src
            .rgb
            .chunks(3)
            .map(|p| [p[0], p[1], p[2]])
            .collect::<Vec<_>>();
        let mut sorted_out = out
            .rgb
            .chunks(3)
            .map(|p| [p[0], p[1], p[2]])
            .collect::<Vec<_>>();
        sorted_src.sort();
        sorted_out.sort();
        assert_eq!(sorted_src, sorted_out, "翻转只能重排像素");
    }

    impl ProjectDoc {
        fn flip_y_for_test(&mut self) {
            self.viewport.flip_y = true;
        }
    }

    #[test]
    fn fills_follow_shape_and_layer_order() {
        let src = source(100, 100);
        let mut d = doc(100, 100);
        d.layers.push(group(
            "a",
            Shape::Rect,
            PixelRect::new(10, 10, 40, 40),
            "#ff0000",
        ));
        d.layers.push(group(
            "b",
            Shape::Ellipse,
            PixelRect::new(30, 30, 40, 40),
            "#0000ff",
        ));
        let mut fonts = Fonts::for_tests();
        let out = compose(&src, &d, &mut fonts).unwrap();
        assert_eq!(px(&out, 12, 12), [255, 0, 0]);
        // The ellipse is on top where they overlap, and its corners stay red.
        assert_eq!(px(&out, 49, 49), [0, 0, 255]);
        assert_eq!(px(&out, 31, 31), [255, 0, 0]);
        // Outside both: source untouched.
        assert_eq!(px(&out, 90, 90), src.pixel(90, 90));

        // Hidden layers do not paint.
        if let Layer::Repair(g) = &mut d.layers[0] {
            g.visible = false;
        }
        let out = compose(&src, &d, &mut fonts).unwrap();
        assert_eq!(px(&out, 12, 12), src.pixel(12, 12));
    }

    #[test]
    fn text_is_painted_when_the_bundled_font_is_available() {
        let mut fonts = Fonts::for_tests();
        if !fonts.bundled_available() {
            return;
        }
        let src = source(200, 100);
        let mut d = doc(200, 100);
        let mut text = sample_text("t", PixelRect::new(10, 10, 180, 60));
        text.color = "#000000".into();
        text.font.size = 30.0;
        d.layers.push(Layer::Text(text));
        let out = compose(&src, &d, &mut fonts).unwrap();
        let dark = out
            .rgb
            .chunks(3)
            .filter(|p| p[0] < 40 && p[1] < 40 && p[2] < 40)
            .count();
        assert!(dark > 200, "文字应当落到输出上，深色像素 {dark}");
        assert!(out.missing_fonts.is_empty());
    }

    #[test]
    fn png_output_is_opaque_rgb_with_whitelisted_metadata_only() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 4, 2);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_pixel_dims(Some(png::PixelDimensions {
                xppu: 11811,
                yppu: 11811,
                unit: png::Unit::Meter,
            }));
            encoder.set_source_srgb(png::SrgbRenderingIntent::RelativeColorimetric);
            encoder
                .add_text_chunk("Author".into(), "secret".into())
                .unwrap();
            let mut writer = encoder.write_header().unwrap();
            let mut data = vec![0u8; 4 * 2 * 4];
            data[3] = 0; // transparent black
            data[7] = 128;
            for px in data.chunks_mut(4).skip(2) {
                px.copy_from_slice(&[10, 20, 30, 255]);
            }
            writer.write_image_data(&data).unwrap();
        }
        let src = decode_bytes(&bytes).unwrap();
        let mut d = doc(4, 2);
        d.viewport.crop = PixelRect::new(0, 0, 2, 1);
        let mut fonts = Fonts::for_tests();
        let out = compose(&src, &d, &mut fonts).unwrap();
        let dir = super::super::asset::tests::temp_dir("png");
        let target = dir.join("out.png");
        write_png(&out, &src, &target).unwrap();

        let decoder = png::Decoder::new(std::io::BufReader::new(
            std::fs::File::open(&target).unwrap(),
        ));
        let mut reader = decoder.read_info().unwrap();
        let info = reader.info().clone();
        assert_eq!(info.color_type, png::ColorType::Rgb);
        assert_eq!((info.width, info.height), (2, 1));
        assert_eq!(info.pixel_dims.map(|d| d.xppu), Some(11811));
        assert_eq!(
            info.srgb,
            Some(png::SrgbRenderingIntent::RelativeColorimetric)
        );
        assert!(
            info.uncompressed_latin1_text.is_empty(),
            "文本块不得带入导出"
        );
        let mut buf = vec![0u8; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut buf).unwrap();
        assert_eq!(&buf[0..3], &[255, 255, 255], "透明像素铺白");
        assert_eq!(&buf[3..6], &[127, 127, 127], "半透明黑与白混合");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_rgb_profiles_are_reported_and_block_export() {
        let mut src = source(2, 2);
        let mut icc = vec![0u8; 128];
        icc[16..20].copy_from_slice(b"CMYK");
        src.icc = Some(icc);
        let d = doc(2, 2);
        let mut fonts = Fonts::for_tests();
        let preview = preview(&src, &d, &mut fonts).unwrap();
        assert!(preview.color_note.is_some());
        let error = compose(&src, &d, &mut fonts).map(|_| ()).unwrap_err();
        assert_eq!(error.code, "workbench_color_profile_unsupported");

        let mut rgb_icc = vec![0u8; 128];
        rgb_icc[16..20].copy_from_slice(b"RGB ");
        src.icc = Some(rgb_icc);
        assert!(preview_note_is_none(&src, &d, &mut fonts));
    }

    fn preview_note_is_none(src: &DecodedSource, d: &ProjectDoc, fonts: &mut Fonts) -> bool {
        preview(src, d, fonts).unwrap().color_note.is_none()
    }
}
