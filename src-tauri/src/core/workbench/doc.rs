//! The versioned project document.
//!
//! This JSON file is the project's source of truth; SQLite only indexes it.
//! Geometry is always integer source pixels (see `geom`), never screen
//! coordinates, and nothing about the preview (zoom, undo queue) is stored.
//! Text layers record what the user chose — font family name, weight, size —
//! plus provenance flags that decide whether automation may overwrite them.

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

use super::geom::PixelRect;

pub const SCHEMA_VERSION: u32 = 1;

/// Longest text a single layer may carry. Keeps a runaway paste from turning
/// the document into something the compositor chokes on.
const MAX_TEXT_CHARS: usize = 4000;
const MAX_LAYERS: usize = 500;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectDoc {
    pub schema_version: u32,
    pub id: String,
    pub revision: u64,
    pub name: String,
    pub source: SourceRef,
    pub viewport: Viewport,
    pub layers: Vec<Layer>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceRef {
    pub asset_id: String,
    pub filename: String,
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_true")]
    pub orientation_normalized: bool,
    #[serde(default = "default_color_space")]
    pub working_color_space: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Viewport {
    pub crop: PixelRect,
    #[serde(default)]
    pub flip_x: bool,
    #[serde(default)]
    pub flip_y: bool,
}

/// A top-level layer. Repair groups are atomic: their fill is always painted
/// first and their child texts always sit directly above it, so ordering only
/// ever moves the whole group.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Layer {
    Repair(RepairGroup),
    Text(TextLayer),
}

impl Layer {
    pub fn id(&self) -> &str {
        match self {
            Layer::Repair(group) => &group.id,
            Layer::Text(text) => &text.id,
        }
    }

    pub fn visible(&self) -> bool {
        match self {
            Layer::Repair(group) => group.visible,
            Layer::Text(text) => text.visible,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairKind {
    Repair,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextKind {
    Text,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    Rect,
    Ellipse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FillSource {
    Auto,
    Manual,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepairGroup {
    pub kind: RepairKind,
    pub id: String,
    #[serde(default = "default_true")]
    pub visible: bool,
    pub shape: Shape,
    pub rect: PixelRect,
    pub fill: Fill,
    #[serde(default)]
    pub ocr: OcrState,
    #[serde(default)]
    pub children: Vec<TextLayer>,
}

/// The fill colour with both the analysed value and the one in use, so a
/// manual choice survives re-analysis and can be reverted.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fill {
    /// Latest automatic candidate, `#rrggbb`. Missing until analysis ran.
    #[serde(default)]
    pub auto: Option<String>,
    /// Colour actually painted, `#rrggbb`.
    pub current: String,
    pub source: FillSource,
    #[serde(default)]
    pub uneven: bool,
    #[serde(default)]
    pub coverage: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OcrState {
    /// `pending` | `none` | `found` | `unreliable` | `unavailable` | `failed`
    #[serde(default)]
    pub status: String,
    /// Set once the user touched any child text or added one by hand;
    /// automation must not replace the children afterwards.
    #[serde(default)]
    pub edited: bool,
    /// The region moved after a manual edit; the texts may no longer match.
    #[serde(default)]
    pub stale: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextLayer {
    pub kind: TextKind,
    pub id: String,
    #[serde(default = "default_true")]
    pub visible: bool,
    pub text: String,
    pub rect: PixelRect,
    pub font: FontSpec,
    /// `#rrggbb`
    pub color: String,
    /// `left` | `center` | `right`
    pub align: String,
    /// `top` | `middle` | `bottom`
    pub valign: String,
    /// Multiplier of the font size.
    pub line_height: f64,
    /// Extra advance per glyph in source pixels.
    #[serde(default)]
    pub letter_spacing: f64,
    #[serde(default = "default_true")]
    pub auto_fit: bool,
    /// Degrees, clockwise, about the box centre. Inherited from OCR only.
    #[serde(default)]
    pub angle: f64,
    /// `ocr` | `manual`
    pub origin: String,
    /// Content, style or box changed by the user.
    #[serde(default)]
    pub edited: bool,
    #[serde(default)]
    pub score: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FontSpec {
    pub family: String,
    pub size: f64,
    pub weight: u16,
    #[serde(default)]
    pub italic: bool,
}

fn default_true() -> bool {
    true
}

fn default_color_space() -> String {
    "srgb".to_string()
}

pub fn parse_hex_color(value: &str) -> Option<[u8; 3]> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

pub fn hex_color(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

impl ProjectDoc {
    /// Reject documents the compositor could not honour. Runs on every save
    /// so a bad in-memory state never becomes the on-disk truth.
    pub fn validate(&self) -> AppResult<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(AppError::with_detail(
                "project_schema_unsupported",
                format!("不支持的工程版本 {}", self.schema_version),
                serde_json::json!({ "supported": SCHEMA_VERSION }),
            ));
        }
        if self.name.trim().is_empty() {
            return Err(AppError::new("project_name_empty", "工程名称不能为空"));
        }
        let (w, h) = (self.source.width, self.source.height);
        if w == 0 || h == 0 {
            return Err(AppError::new("project_source_invalid", "源图尺寸无效"));
        }
        if !self.viewport.crop.within(w, h) {
            return Err(AppError::new("project_crop_invalid", "裁剪范围超出源图"));
        }
        if self.layers.len() > MAX_LAYERS {
            return Err(AppError::new("project_too_many_layers", "图层数量超过上限"));
        }
        let mut ids = std::collections::HashSet::new();
        for layer in &self.layers {
            match layer {
                Layer::Repair(group) => {
                    if !ids.insert(group.id.as_str()) {
                        return Err(AppError::new("project_duplicate_layer", "图层 id 重复"));
                    }
                    if !group.rect.within(w, h) {
                        return Err(AppError::new(
                            "project_layer_out_of_bounds",
                            "修补区域超出源图",
                        ));
                    }
                    if parse_hex_color(&group.fill.current).is_none() {
                        return Err(AppError::new("project_color_invalid", "填充色格式无效"));
                    }
                    for child in &group.children {
                        if !ids.insert(child.id.as_str()) {
                            return Err(AppError::new("project_duplicate_layer", "图层 id 重复"));
                        }
                        child.validate(w, h)?;
                    }
                }
                Layer::Text(text) => {
                    if !ids.insert(text.id.as_str()) {
                        return Err(AppError::new("project_duplicate_layer", "图层 id 重复"));
                    }
                    text.validate(w, h)?;
                }
            }
        }
        Ok(())
    }
}

impl TextLayer {
    fn validate(&self, w: u32, h: u32) -> AppResult<()> {
        if !self.rect.is_valid() || self.rect.clamped(w, h).is_none() {
            return Err(AppError::new(
                "project_layer_out_of_bounds",
                "文本框超出源图",
            ));
        }
        if self.text.chars().count() > MAX_TEXT_CHARS {
            return Err(AppError::new("project_text_too_long", "文本内容过长"));
        }
        if parse_hex_color(&self.color).is_none() {
            return Err(AppError::new("project_color_invalid", "文字颜色格式无效"));
        }
        if !(self.font.size.is_finite() && self.font.size >= 1.0 && self.font.size <= 4096.0) {
            return Err(AppError::new("project_font_size_invalid", "字号无效"));
        }
        if !(100..=900).contains(&self.font.weight) {
            return Err(AppError::new("project_font_weight_invalid", "字重无效"));
        }
        if !(self.line_height.is_finite() && self.line_height >= 0.5 && self.line_height <= 4.0) {
            return Err(AppError::new("project_line_height_invalid", "行距无效"));
        }
        if !self.letter_spacing.is_finite() || self.letter_spacing.abs() > 512.0 {
            return Err(AppError::new(
                "project_letter_spacing_invalid",
                "字间距无效",
            ));
        }
        if !matches!(self.align.as_str(), "left" | "center" | "right") {
            return Err(AppError::new("project_align_invalid", "对齐方式无效"));
        }
        if !matches!(self.valign.as_str(), "top" | "middle" | "bottom") {
            return Err(AppError::new("project_align_invalid", "垂直对齐方式无效"));
        }
        if !self.angle.is_finite() || self.angle.abs() > 360.0 {
            return Err(AppError::new("project_angle_invalid", "文字角度无效"));
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn sample_text(id: &str, rect: PixelRect) -> TextLayer {
    TextLayer {
        kind: TextKind::Text,
        id: id.to_string(),
        visible: true,
        text: "样本 Sample 95.2%".to_string(),
        rect,
        font: FontSpec {
            family: "Noto Sans SC".to_string(),
            size: 24.0,
            weight: 400,
            italic: false,
        },
        color: "#1d2029".to_string(),
        align: "left".to_string(),
        valign: "top".to_string(),
        line_height: 1.2,
        letter_spacing: 0.0,
        auto_fit: true,
        angle: 0.0,
        origin: "manual".to_string(),
        edited: false,
        score: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> ProjectDoc {
        ProjectDoc {
            schema_version: SCHEMA_VERSION,
            id: "p1".into(),
            revision: 1,
            name: "图 · 编辑 1".into(),
            source: SourceRef {
                asset_id: "a1".into(),
                filename: "figure.png".into(),
                width: 200,
                height: 100,
                orientation_normalized: true,
                working_color_space: "srgb".into(),
            },
            viewport: Viewport {
                crop: PixelRect::new(0, 0, 200, 100),
                flip_x: false,
                flip_y: false,
            },
            layers: vec![
                Layer::Repair(RepairGroup {
                    kind: RepairKind::Repair,
                    id: "g1".into(),
                    visible: true,
                    shape: Shape::Ellipse,
                    rect: PixelRect::new(10, 10, 50, 20),
                    fill: Fill {
                        auto: Some("#ffffff".into()),
                        current: "#ffffff".into(),
                        source: FillSource::Auto,
                        uneven: false,
                        coverage: Some(0.9),
                    },
                    ocr: OcrState::default(),
                    children: vec![sample_text("t1", PixelRect::new(12, 12, 40, 16))],
                }),
                Layer::Text(sample_text("t2", PixelRect::new(100, 50, 60, 20))),
            ],
            created_at: "2026-09-05T00:00:00Z".into(),
            updated_at: "2026-09-05T00:00:00Z".into(),
        }
    }

    #[test]
    fn round_trips_through_json_with_kind_tags() {
        let json = serde_json::to_string(&doc()).unwrap();
        assert!(json.contains("\"kind\":\"repair\""));
        assert!(json.contains("\"kind\":\"text\""));
        let back: ProjectDoc = serde_json::from_str(&json).unwrap();
        assert!(matches!(back.layers[0], Layer::Repair(_)));
        assert!(matches!(back.layers[1], Layer::Text(_)));
        back.validate().unwrap();
    }

    #[test]
    fn rejects_out_of_bounds_and_unknown_schema() {
        let mut bad = doc();
        bad.viewport.crop = PixelRect::new(0, 0, 201, 100);
        assert_eq!(bad.validate().unwrap_err().code, "project_crop_invalid");

        let mut bad = doc();
        if let Layer::Repair(group) = &mut bad.layers[0] {
            group.rect = PixelRect::new(190, 90, 20, 20);
        }
        assert_eq!(
            bad.validate().unwrap_err().code,
            "project_layer_out_of_bounds"
        );

        let mut bad = doc();
        bad.schema_version = 99;
        assert_eq!(
            bad.validate().unwrap_err().code,
            "project_schema_unsupported"
        );

        let mut bad = doc();
        if let Layer::Text(text) = &mut bad.layers[1] {
            text.id = "g1".into();
        }
        assert_eq!(bad.validate().unwrap_err().code, "project_duplicate_layer");
    }

    #[test]
    fn hex_colors_parse_and_format() {
        assert_eq!(parse_hex_color("#1A2b3C"), Some([0x1a, 0x2b, 0x3c]));
        assert_eq!(parse_hex_color("1a2b3c"), None);
        assert_eq!(parse_hex_color("#12345"), None);
        assert_eq!(hex_color([255, 0, 16]), "#ff0010");
    }
}
