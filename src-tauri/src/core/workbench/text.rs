//! Fonts, text measurement and glyph rasterization.
//!
//! One `FontSystem` per process: building it scans every system font and
//! takes long enough that doing it per request would make the first
//! keystroke in a text box visibly stall. The bundled Noto Sans SC is the
//! fallback of last resort so an exported project renders on a machine that
//! lacks the chosen family; the project keeps the original family name and
//! the layout reports the substitution instead of hiding it.

use std::path::Path;
use std::sync::Mutex;

use cosmic_text::{
    Align, Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Style, SwashCache, Weight, Wrap,
};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// Family name of the font shipped with the app.
pub const BUNDLED_FAMILY: &str = "Noto Sans SC";
pub const BUNDLED_FILE: &str = "NotoSansSC-Regular.otf";

/// Smallest size auto-fit will shrink to before giving up and overflowing.
const MIN_FIT_SIZE: f64 = 4.0;

#[derive(Clone, Debug, Deserialize)]
pub struct TextSpec {
    pub text: String,
    pub width: f64,
    pub height: f64,
    pub family: String,
    pub size: f64,
    pub weight: u16,
    #[serde(default)]
    pub italic: bool,
    pub line_height: f64,
    #[serde(default)]
    pub letter_spacing: f64,
    pub align: String,
    pub valign: String,
    #[serde(default)]
    pub auto_fit: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct LineBox {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct TextLayout {
    pub lines: Vec<LineBox>,
    /// Content size after wrapping, before vertical alignment.
    pub content_width: f64,
    pub content_height: f64,
    /// Vertical offset applied by `valign`, never negative.
    pub offset_y: f64,
    /// Size actually used (smaller than requested only with auto-fit).
    pub font_size: f64,
    pub overflow: bool,
    pub family_used: String,
    pub missing_font: bool,
    /// Horizontal emboldening in source pixels when the requested weight is
    /// bold but the matched face has no bold cut (many CJK families ship one
    /// weight). Zero when a real bold face was used. The preview strokes the
    /// glyphs by this amount so it matches the export.
    pub synthetic_bold: f64,
    /// Shear (tan of the slant angle) applied around each baseline when italic
    /// was requested but the matched face is upright. Zero when a real italic
    /// face was used. The preview skews its text nodes by the same amount.
    pub synthetic_italic: f64,
}

/// Slant used for faux italics: 14°, the angle WebKit and Skia use.
pub const SYNTHETIC_ITALIC_SKEW: f64 = 0.249_328;

/// Upper bound for auto-fit growth: one line can never be taller than the box,
/// so the size that makes a single line fill the height caps the search.
fn auto_fit_ceiling(spec: &TextSpec) -> f64 {
    (spec.height / spec.line_height.clamp(0.5, 4.0)).clamp(1.0, 4096.0)
}

/// How much to embolden glyphs for `requested` weight when the face that was
/// actually matched weighs `face` (fontdb scale). Same shape as the fake-bold
/// outset Skia uses (about `size / 32` for a 700 request on a regular face),
/// never below one pixel so small text still changes visibly.
pub fn synthetic_bold_px(size: f64, requested: u16, face: u16) -> f64 {
    if requested < 600 || face + 100 >= requested {
        return 0.0;
    }
    (size / 32.0 * f64::from(requested - face) / 300.0).clamp(1.0, 6.0)
}

#[derive(Clone, Debug, Serialize)]
pub struct FontInfo {
    pub family: String,
    /// Whether every character of the sample has a glyph. `None` when no
    /// sample was given.
    pub covers: Option<bool>,
    pub bundled: bool,
}

pub struct Fonts {
    system: FontSystem,
    cache: SwashCache,
    bundled: bool,
}

impl Fonts {
    /// Load system fonts plus the bundled fallback from `font_root`.
    pub fn load(font_root: &Path) -> Self {
        let mut db = cosmic_text::fontdb::Database::new();
        db.load_system_fonts();
        let bundled_path = font_root.join(BUNDLED_FILE);
        let bundled = match std::fs::read(&bundled_path) {
            Ok(bytes) => {
                db.load_font_data(bytes);
                true
            }
            Err(_) => false,
        };
        let locale = sys_locale::get_locale().unwrap_or_else(|| "zh-CN".to_string());
        Self {
            system: FontSystem::new_with_locale_and_db(locale, db),
            cache: SwashCache::new(),
            bundled,
        }
    }

    #[cfg(test)]
    pub fn for_tests() -> Self {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fonts")
            .join(BUNDLED_FILE);
        let mut db = cosmic_text::fontdb::Database::new();
        db.load_font_data(std::fs::read(path).expect("测试需要仓库内置字体"));
        Self {
            system: FontSystem::new_with_locale_and_db("zh-CN".into(), db),
            cache: SwashCache::new(),
            bundled: true,
        }
    }

    pub fn bundled_available(&self) -> bool {
        self.bundled
    }

    fn has_family(&self, family: &str) -> bool {
        let wanted = family.trim().to_lowercase();
        self.system.db().faces().any(|face| {
            face.families
                .iter()
                .any(|(name, _)| name.to_lowercase() == wanted)
        })
    }

    /// Resolve the family to use and whether that is a substitution.
    fn resolve_family(&self, family: &str) -> (String, bool) {
        if !family.trim().is_empty() && self.has_family(family) {
            return (family.trim().to_string(), false);
        }
        if self.bundled {
            return (BUNDLED_FAMILY.to_string(), !family.trim().is_empty());
        }
        (String::new(), !family.trim().is_empty())
    }

    /// Unique family names, with coverage for `sample` if given. The bundled
    /// family is listed first, then the rest alphabetically.
    pub fn list(&mut self, sample: Option<&str>) -> Vec<FontInfo> {
        let sample_chars: Vec<char> = sample
            .map(|text| {
                let mut chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
                chars.sort_unstable();
                chars.dedup();
                chars.truncate(200);
                chars
            })
            .unwrap_or_default();
        let mut families: std::collections::BTreeMap<String, cosmic_text::fontdb::ID> =
            std::collections::BTreeMap::new();
        for face in self.system.db().faces() {
            let Some((name, _)) = face.families.first() else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            families.entry(name.clone()).or_insert(face.id);
        }
        let mut out = Vec::with_capacity(families.len());
        for (family, id) in families {
            let covers = if sample_chars.is_empty() {
                None
            } else {
                Some(match self.system.get_font(id, Weight::NORMAL) {
                    Some(font) => {
                        let charmap = font.as_swash().charmap();
                        sample_chars.iter().all(|c| charmap.map(*c) != 0)
                    }
                    None => false,
                })
            };
            out.push(FontInfo {
                bundled: family == BUNDLED_FAMILY,
                family,
                covers,
            });
        }
        out.sort_by(|a, b| {
            b.bundled
                .cmp(&a.bundled)
                .then_with(|| a.family.cmp(&b.family))
        });
        out
    }

    /// Lay out `spec` and describe the result without rasterizing.
    pub fn measure(&mut self, spec: &TextSpec) -> AppResult<TextLayout> {
        let (buffer, layout) = self.layout(spec)?;
        drop(buffer);
        Ok(layout)
    }

    fn layout(&mut self, spec: &TextSpec) -> AppResult<(Buffer, TextLayout)> {
        if !(spec.size.is_finite() && spec.size >= 1.0)
            || !(spec.width >= 1.0 && spec.height >= 1.0)
        {
            return Err(AppError::new("text_spec_invalid", "文本参数无效"));
        }
        let (family, missing) = self.resolve_family(&spec.family);
        let mut size = spec.size;
        let mut attempt = self.shape(spec, &family, size);
        if spec.auto_fit {
            // Auto-fit picks the largest size that fits the box — growing into
            // free space as well as shrinking out of overflow. Sizes stay on a
            // quarter-pixel grid so preview and export ask for the same one.
            let min_size = MIN_FIT_SIZE.min(spec.size);
            let max_size = auto_fit_ceiling(spec).max(spec.size);
            let quarter = |value: f64| (value * 4.0).round() / 4.0;
            let (mut lo, mut hi, mut best) = if attempt.1.overflow {
                (min_size, size, None)
            } else {
                (size, max_size, Some(attempt))
            };
            if best.is_some() && hi > lo {
                let top = self.shape(spec, &family, hi);
                if !top.1.overflow {
                    lo = hi;
                    best = Some(top);
                }
            }
            for _ in 0..14 {
                let mid = quarter((lo + hi) / 2.0);
                if mid <= lo || mid >= hi {
                    break;
                }
                let candidate = self.shape(spec, &family, mid);
                if candidate.1.overflow {
                    hi = mid;
                } else {
                    lo = mid;
                    best = Some(candidate);
                }
            }
            attempt = match best {
                Some(found) => found,
                None => self.shape(spec, &family, min_size),
            };
            size = attempt.1.font_size;
        }
        let (buffer, mut layout) = attempt;
        layout.font_size = size;
        layout.family_used = family;
        layout.missing_font = missing;
        Ok((buffer, layout))
    }

    fn shape(&mut self, spec: &TextSpec, family: &str, size: f64) -> (Buffer, TextLayout) {
        let line_height = (size * spec.line_height.clamp(0.5, 4.0)) as f32;
        let mut buffer = Buffer::new(&mut self.system, Metrics::new(size as f32, line_height));
        buffer.set_size(Some(spec.width as f32), None);
        buffer.set_wrap(Wrap::WordOrGlyph);
        let mut attrs = Attrs::new()
            .weight(Weight(spec.weight.clamp(100, 900)))
            .style(if spec.italic {
                Style::Italic
            } else {
                Style::Normal
            });
        if !family.is_empty() {
            attrs = attrs.family(Family::Name(family));
        }
        if spec.letter_spacing != 0.0 {
            attrs = attrs.letter_spacing((spec.letter_spacing / size) as f32);
        }
        let align = match spec.align.as_str() {
            "center" => Align::Center,
            "right" => Align::Right,
            _ => Align::Left,
        };
        buffer.set_text(&spec.text, &attrs, Shaping::Advanced, Some(align));
        buffer.shape_until_scroll(&mut self.system, false);

        // The face fontdb matched for the first glyph tells whether bold or
        // italic requests were honoured by real cuts or need synthesis.
        let requested_weight = spec.weight.clamp(100, 900);
        let matched = buffer
            .layout_runs()
            .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.font_id))
            .next()
            .and_then(|id| {
                self.system
                    .db()
                    .face(id)
                    .map(|face| (face.weight.0, face.style))
            });
        let face_weight = matched
            .map(|(weight, _)| weight)
            .unwrap_or(requested_weight);
        let face_upright =
            matched.is_some_and(|(_, style)| style == cosmic_text::fontdb::Style::Normal);
        let synthetic_bold = synthetic_bold_px(size, requested_weight, face_weight);
        let synthetic_italic = if spec.italic && face_upright {
            SYNTHETIC_ITALIC_SKEW
        } else {
            0.0
        };

        let mut lines = Vec::new();
        let mut content_width: f64 = 0.0;
        let mut content_height: f64 = 0.0;
        for run in buffer.layout_runs() {
            let start = run.glyphs.first().map(|g| g.start).unwrap_or(0);
            let end = run.glyphs.last().map(|g| g.end).unwrap_or(start);
            let text = run.text.get(start..end).unwrap_or("").to_string();
            let x = run.glyphs.iter().map(|g| g.x).fold(f32::INFINITY, f32::min);
            let x = if x.is_finite() { x } else { 0.0 };
            lines.push(LineBox {
                text,
                x: f64::from(x),
                y: f64::from(run.line_top),
                width: f64::from(run.line_w),
                height: f64::from(run.line_height),
            });
            content_width = content_width.max(f64::from(run.line_w));
            content_height = content_height.max(f64::from(run.line_top + run.line_height));
        }
        content_width += synthetic_bold + synthetic_italic * size;
        let overflow = content_height > spec.height + 0.5 || content_width > spec.width + 0.5;
        let offset_y = match spec.valign.as_str() {
            "middle" => ((spec.height - content_height) / 2.0).max(0.0),
            "bottom" => (spec.height - content_height).max(0.0),
            _ => 0.0,
        };
        let layout = TextLayout {
            lines,
            content_width,
            content_height,
            offset_y,
            font_size: size,
            overflow,
            family_used: family.to_string(),
            missing_font: false,
            synthetic_bold,
            synthetic_italic,
        };
        (buffer, layout)
    }

    /// Rasterize `spec` in `color` into an RGBA (premultiplied) buffer of
    /// `width`×`height` pixels whose origin is the text box's top-left.
    /// Glyphs that overflow the box are kept (the buffer is padded), so the
    /// caller decides whether to clip.
    pub fn rasterize(
        &mut self,
        spec: &TextSpec,
        color: [u8; 3],
    ) -> AppResult<(tiny_skia::Pixmap, TextLayout, i32, i32)> {
        let (mut buffer, layout) = self.layout(spec)?;
        let pad = (layout.font_size * 1.5).ceil() as i32;
        let width = (spec.width.ceil() as i32 + pad * 2).max(1);
        let height = (spec
            .height
            .max(layout.content_height + layout.offset_y)
            .ceil() as i32
            + pad * 2)
            .max(1);
        let mut pixmap = tiny_skia::Pixmap::new(width as u32, height as u32)
            .ok_or_else(|| AppError::new("text_raster_failed", "文字缓冲区创建失败"))?;
        let pixels = pixmap.data_mut();
        let stride = width as usize * 4;
        let origin_x = pad;
        let origin_y = pad + layout.offset_y.round() as i32;
        let base = cosmic_text::Color::rgb(color[0], color[1], color[2]);
        // Faux italic shears every pixel around its own line's baseline.
        let skew = layout.synthetic_italic as f32;
        let baselines: Vec<(i32, i32, f32)> = buffer
            .layout_runs()
            .map(|run| {
                (
                    run.line_top.floor() as i32,
                    (run.line_top + run.line_height).ceil() as i32,
                    run.line_y,
                )
            })
            .collect();
        let shear = |py: i32| -> i32 {
            if skew == 0.0 {
                return 0;
            }
            let baseline = baselines
                .iter()
                .find(|(top, bottom, _)| py >= *top && py < *bottom)
                .or(baselines.last())
                .map(|(_, _, baseline)| *baseline)
                .unwrap_or(0.0);
            ((baseline - py as f32) * skew).round() as i32
        };
        // Synthetic bold: overprint the run shifted right by one pixel per pass,
        // the same fake-bold technique browsers use for faces without a bold cut.
        let passes = layout.synthetic_bold.round().max(0.0) as i32;
        for shift in 0..=passes {
            buffer.draw(&mut self.system, &mut self.cache, base, |x, y, w, h, c| {
                let alpha = u32::from(c.a());
                if alpha == 0 {
                    return;
                }
                for py in y..y + h as i32 {
                    let slant = shear(py);
                    for px in x..x + w as i32 {
                        let (tx, ty) = (px + origin_x + shift + slant, py + origin_y);
                        if tx < 0 || ty < 0 || tx >= width || ty >= height {
                            continue;
                        }
                        let i = ty as usize * stride + tx as usize * 4;
                        // Source-over in premultiplied space onto whatever glyph
                        // already covers this pixel.
                        let inv = 255 - alpha;
                        let sr = u32::from(c.r()) * alpha / 255;
                        let sg = u32::from(c.g()) * alpha / 255;
                        let sb = u32::from(c.b()) * alpha / 255;
                        pixels[i] = (sr + u32::from(pixels[i]) * inv / 255) as u8;
                        pixels[i + 1] = (sg + u32::from(pixels[i + 1]) * inv / 255) as u8;
                        pixels[i + 2] = (sb + u32::from(pixels[i + 2]) * inv / 255) as u8;
                        pixels[i + 3] = (alpha + u32::from(pixels[i + 3]) * inv / 255) as u8;
                    }
                }
            });
        }
        Ok((pixmap, layout, -origin_x, -pad))
    }
}

/// Shared, lazily built font state.
#[derive(Default)]
pub struct FontHandle {
    fonts: Mutex<Option<Fonts>>,
}

impl FontHandle {
    pub fn with<T>(&self, font_root: &Path, f: impl FnOnce(&mut Fonts) -> T) -> T {
        let mut guard = self
            .fonts
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let fonts = guard.get_or_insert_with(|| Fonts::load(font_root));
        f(fonts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(text: &str, width: f64, height: f64, size: f64) -> TextSpec {
        TextSpec {
            text: text.to_string(),
            width,
            height,
            family: BUNDLED_FAMILY.to_string(),
            size,
            weight: 400,
            italic: false,
            line_height: 1.2,
            letter_spacing: 0.0,
            align: "left".into(),
            valign: "top".into(),
            auto_fit: false,
        }
    }

    #[test]
    fn measures_wrapping_and_auto_fit() {
        let mut fonts = Fonts::for_tests();
        if !fonts.bundled_available() {
            eprintln!("bundled font missing; skipping");
            return;
        }
        let one = fonts
            .measure(&spec("准确率 95.2%", 400.0, 60.0, 24.0))
            .unwrap();
        assert_eq!(one.lines.len(), 1);
        assert!(!one.overflow);
        assert!(!one.missing_font);
        assert_eq!(one.family_used, BUNDLED_FAMILY);

        let narrow = fonts
            .measure(&spec(
                "准确率 95.2% Model V2 样本数：100",
                120.0,
                30.0,
                24.0,
            ))
            .unwrap();
        assert!(narrow.lines.len() > 1);
        assert!(narrow.overflow, "两行以上必然超过 30px 高");

        let mut fitted = spec("准确率 95.2% Model V2 样本数：100", 120.0, 30.0, 24.0);
        fitted.auto_fit = true;
        let fit = fonts.measure(&fitted).unwrap();
        assert!(fit.font_size < 24.0);
        assert!(!fit.overflow, "自动适配后不应溢出: {fit:?}");

        // Growth: a short label in a roomy box fills the box instead of
        // staying at the requested size, and stays on the quarter-pixel grid.
        let mut roomy = spec("Ab", 400.0, 120.0, 12.0);
        roomy.auto_fit = true;
        let grown = fonts.measure(&roomy).unwrap();
        assert!(grown.font_size > 12.0, "{grown:?}");
        assert!(
            grown.font_size <= 100.0,
            "不能超过 height / line_height: {grown:?}"
        );
        assert!(!grown.overflow);
        assert_eq!(grown.font_size * 4.0, (grown.font_size * 4.0).round());
        let mut fixed = roomy.clone();
        fixed.auto_fit = false;
        assert_eq!(
            fonts.measure(&fixed).unwrap().font_size,
            12.0,
            "关闭自动适配保留手动字号"
        );
    }

    #[test]
    fn missing_family_falls_back_to_the_bundled_font_and_says_so() {
        let mut fonts = Fonts::for_tests();
        if !fonts.bundled_available() {
            return;
        }
        let mut s = spec("Hello 世界", 300.0, 40.0, 20.0);
        s.family = "Definitely Not Installed Font 9000".into();
        let layout = fonts.measure(&s).unwrap();
        assert!(layout.missing_font);
        assert_eq!(layout.family_used, BUNDLED_FAMILY);
    }

    #[test]
    fn vertical_alignment_offsets_are_clamped() {
        let mut fonts = Fonts::for_tests();
        if !fonts.bundled_available() {
            return;
        }
        let mut s = spec("A", 100.0, 100.0, 20.0);
        s.valign = "middle".into();
        let middle = fonts.measure(&s).unwrap();
        assert!(middle.offset_y > 30.0 && middle.offset_y < 45.0);
        s.valign = "bottom".into();
        s.height = 10.0;
        let bottom = fonts.measure(&s).unwrap();
        assert_eq!(bottom.offset_y, 0.0);
        assert!(bottom.overflow);
    }

    #[test]
    fn rasterizes_visible_glyphs() {
        let mut fonts = Fonts::for_tests();
        if !fonts.bundled_available() {
            return;
        }
        let (pixmap, layout, _, _) = fonts
            .rasterize(&spec("样本", 200.0, 50.0, 32.0), [0, 0, 0])
            .unwrap();
        assert!(!layout.overflow);
        let opaque = pixmap.data().chunks(4).filter(|px| px[3] > 128).count();
        assert!(opaque > 100, "应当有实际字形像素，得到 {opaque}");
    }

    #[test]
    fn synthetic_bold_only_when_no_bold_face_answers() {
        assert_eq!(synthetic_bold_px(111.0, 400, 400), 0.0);
        assert_eq!(synthetic_bold_px(111.0, 700, 700), 0.0, "真实粗体不加粗");
        assert_eq!(
            synthetic_bold_px(111.0, 700, 600),
            0.0,
            "相邻字重视为已满足"
        );
        assert!((synthetic_bold_px(111.0, 700, 400) - 3.47).abs() < 0.01);
        assert!(synthetic_bold_px(111.0, 800, 300) > synthetic_bold_px(111.0, 700, 400));
        assert_eq!(synthetic_bold_px(12.0, 700, 400), 1.0, "小字号至少 1 px");
        assert_eq!(synthetic_bold_px(4000.0, 900, 100), 6.0, "上限 6 px");
    }

    /// The bundled Noto Sans SC Regular has no bold cut, so a bold request must
    /// embolden: the rasterized glyphs cover more pixels than the regular ones.
    #[test]
    fn bold_request_on_single_weight_family_emboldens_output() {
        let mut fonts = Fonts::for_tests();
        if !fonts.bundled_available() {
            return;
        }
        let regular = spec("样本 Bold", 300.0, 60.0, 32.0);
        let mut bold = regular.clone();
        bold.weight = 700;
        let (regular_px, regular_layout, _, _) = fonts.rasterize(&regular, [0, 0, 0]).unwrap();
        let (bold_px, bold_layout, _, _) = fonts.rasterize(&bold, [0, 0, 0]).unwrap();
        assert_eq!(regular_layout.synthetic_bold, 0.0);
        assert!(bold_layout.synthetic_bold >= 1.0, "{bold_layout:?}");
        let ink =
            |pixmap: &tiny_skia::Pixmap| pixmap.data().chunks(4).filter(|px| px[3] > 64).count();
        assert!(
            ink(&bold_px) > ink(&regular_px) * 11 / 10,
            "加粗后墨迹应明显增加"
        );
    }

    /// Noto Sans SC has no italic cut either: an italic request must shear the
    /// glyphs. Pixels above the baseline move right, so the ink's horizontal
    /// centre of mass in the top half ends up right of the bottom half's.
    #[test]
    fn italic_request_on_upright_family_shears_output() {
        let mut fonts = Fonts::for_tests();
        if !fonts.bundled_available() {
            return;
        }
        let upright = spec("HHHH", 300.0, 80.0, 48.0);
        let mut italic = upright.clone();
        italic.italic = true;
        let (upright_px, upright_layout, _, _) = fonts.rasterize(&upright, [0, 0, 0]).unwrap();
        let (italic_px, italic_layout, _, _) = fonts.rasterize(&italic, [0, 0, 0]).unwrap();
        assert_eq!(upright_layout.synthetic_italic, 0.0);
        assert!((italic_layout.synthetic_italic - SYNTHETIC_ITALIC_SKEW).abs() < 1e-9);
        let lean = |pixmap: &tiny_skia::Pixmap| -> f64 {
            let width = pixmap.width() as usize;
            let height = pixmap.height() as usize;
            let (mut top_x, mut top_n, mut bottom_x, mut bottom_n) =
                (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
            for (index, px) in pixmap.data().chunks(4).enumerate() {
                if px[3] <= 64 {
                    continue;
                }
                let (x, y) = ((index % width) as f64, index / width);
                if y < height / 2 {
                    top_x += x;
                    top_n += 1.0;
                } else {
                    bottom_x += x;
                    bottom_n += 1.0;
                }
            }
            top_x / top_n.max(1.0) - bottom_x / bottom_n.max(1.0)
        };
        assert!(lean(&upright_px).abs() < 1.0, "直立字形上下重心应对齐");
        assert!(
            lean(&italic_px) > 3.0,
            "倾斜后上半部分应明显右移: {}",
            lean(&italic_px)
        );
    }

    #[test]
    fn lists_families_with_coverage() {
        let mut fonts = Fonts::for_tests();
        let list = fonts.list(Some("样本 Sample"));
        assert!(!list.is_empty());
        if fonts.bundled_available() {
            assert_eq!(list[0].family, BUNDLED_FAMILY);
            assert_eq!(list[0].covers, Some(true));
        }
    }
}
