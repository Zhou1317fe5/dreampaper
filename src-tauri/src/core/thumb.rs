//! On-demand downscales for the image protocol.
//!
//! Generated figures are saved at model resolution — 5504x3072 is typical,
//! roughly 68MB once a browser decodes it. The history grid paints them into
//! ~400px cards, so serving the originals there made every visible card pay a
//! full-resolution decode, and switching a filter re-paid it for every card
//! that remounted. A `?w=` query on the asset URL gets a cached JPEG instead.
//!
//! Everything here degrades to the original file rather than failing: a
//! thumbnail is an optimization, and losing it must never hide an image.

use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};

use image::imageops::FilterType;
use image::{ExtendedColorType, ImageEncoder, ImageReader};

use crate::error::{AppError, AppResult};

/// Widths we are willing to produce. A closed set stops an arbitrary query
/// from making the app decode and cache sizes nobody asked for.
const ALLOWED_WIDTHS: &[u32] = &[400, 800, 1600];

/// Quality is deliberately high: these are research figures, and ringing
/// around thin lines and small labels is more noticeable than the file size.
const JPEG_QUALITY: u8 = 85;

/// Reads a supported `w=` out of a URI query. `None` means "serve the
/// original", including when the width is one we do not offer.
pub fn requested_width(query: Option<&str>) -> Option<u32> {
    let width: u32 = query?
        .split('&')
        .find_map(|pair| pair.strip_prefix("w="))?
        .parse()
        .ok()?;
    ALLOWED_WIDTHS.contains(&width).then_some(width)
}

/// A cached downscale of `source`, built on first request.
///
/// Returns the path to serve and whether it is a generated JPEG. A `false`
/// flag means the caller should keep the source's own content type — the
/// image was already small enough, or could not be decoded at all.
pub fn scaled(app_data: &Path, id: &str, source: &Path, width: u32) -> (PathBuf, bool) {
    match build(app_data, id, source, width) {
        Ok(Some(path)) => (path, true),
        Ok(None) | Err(_) => (source.to_path_buf(), false),
    }
}

/// `Ok(None)` means the source needs no downscale and should be served as-is.
fn build(app_data: &Path, id: &str, source: &Path, width: u32) -> AppResult<Option<PathBuf>> {
    let dir = app_data.join("thumbs");
    let target = dir.join(format!("{id}_{width}.jpg"));
    if is_current(&target, source) {
        return Ok(Some(target));
    }

    let _permit = Permit::acquire();
    // The queue we just waited in may have contained a request for this very
    // file, so look again before paying for the decode.
    if is_current(&target, source) {
        return Ok(Some(target));
    }

    let decoded = ImageReader::open(source)
        .map_err(|error| AppError::new("thumbnail_open_failed", error.to_string()))?
        .with_guessed_format()
        .map_err(|error| AppError::new("thumbnail_format_failed", error.to_string()))?
        .decode()
        .map_err(|error| AppError::new("thumbnail_decode_failed", error.to_string()))?;

    let (source_width, source_height) = (decoded.width(), decoded.height());
    if source_width == 0 || source_height == 0 || source_width <= width {
        return Ok(None);
    }
    // Derive the height ourselves so the aspect ratio cannot drift with how
    // resize() happens to round its bounding box.
    let height = ((u64::from(source_height) * u64::from(width)) / u64::from(source_width)).max(1);
    let height = u32::try_from(height).unwrap_or(1);

    let rgb = decoded.resize_exact(width, height, FilterType::Lanczos3).into_rgb8();
    let mut encoded = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, JPEG_QUALITY)
        .write_image(rgb.as_raw(), rgb.width(), rgb.height(), ExtendedColorType::Rgb8)
        .map_err(|error| AppError::new("thumbnail_encode_failed", error.to_string()))?;

    std::fs::create_dir_all(&dir)?;
    // Write beside the target and rename: two requests can race for the same
    // thumbnail, and a half-written cache entry would be served forever.
    let staging = dir.join(format!("{id}_{width}.{}.part", uuid::Uuid::new_v4()));
    std::fs::write(&staging, &encoded)?;
    if let Err(error) = publish(&staging, &target) {
        let _ = std::fs::remove_file(&staging);
        return Err(error.into());
    }
    Ok(Some(target))
}

/// Moves the finished thumbnail into place.
///
/// Retried because on Windows a rename fails outright while anything else holds
/// the file — an antivirus or search indexer opening what we just wrote is
/// enough — and giving up would drop this card back to the full-resolution
/// image for the rest of the session.
fn publish(staging: &Path, target: &Path) -> std::io::Result<()> {
    let mut last = match std::fs::rename(staging, target) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    for backoff_ms in [20, 60, 150] {
        std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
        match std::fs::rename(staging, target) {
            Ok(()) => return Ok(()),
            Err(error) => last = error,
        }
    }
    Err(last)
}

/// A cached thumbnail is stale once the file it came from is newer.
fn is_current(target: &Path, source: &Path) -> bool {
    let (Ok(cached), Ok(original)) = (target.metadata(), source.metadata()) else {
        return false;
    };
    match (cached.modified(), original.modified()) {
        (Ok(cached_at), Ok(original_at)) => cached_at >= original_at,
        _ => false,
    }
}

/// Building one thumbnail holds a full-resolution decode in memory — ~68MB for
/// a 5504x3072 figure. The webview happily asks for a whole gridful at once, so
/// the cap belongs here: how many run together is our decision, not the page's.
struct Permit;

struct Gate {
    free: Mutex<usize>,
    released: Condvar,
}

fn gate() -> &'static Gate {
    static GATE: OnceLock<Gate> = OnceLock::new();
    GATE.get_or_init(|| Gate {
        free: Mutex::new(
            std::thread::available_parallelism()
                .map(|cores| cores.get() / 2)
                .unwrap_or(2)
                .clamp(1, 4),
        ),
        released: Condvar::new(),
    })
}

impl Permit {
    fn acquire() -> Self {
        let gate = gate();
        // A panic mid-resize must not wedge every later thumbnail, so a
        // poisoned gate is recovered rather than propagated.
        let mut free = gate.free.lock().unwrap_or_else(|error| error.into_inner());
        while *free == 0 {
            free = gate
                .released
                .wait(free)
                .unwrap_or_else(|error| error.into_inner());
        }
        *free -= 1;
        Permit
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let gate = gate();
        let mut free = gate.free.lock().unwrap_or_else(|error| error.into_inner());
        *free += 1;
        gate.released.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thumb_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dreampaper-thumb-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    fn write_png(path: &Path, width: u32, height: u32) {
        let buffer = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        buffer.save(path).expect("写测试 PNG");
    }

    fn dimensions(path: &Path) -> (u32, u32) {
        let decoded = ImageReader::open(path)
            .expect("打开图片")
            .with_guessed_format()
            .expect("识别格式")
            .decode()
            .expect("解码图片");
        (decoded.width(), decoded.height())
    }

    #[test]
    fn only_offered_widths_are_accepted() {
        assert_eq!(requested_width(Some("w=800")), Some(800));
        assert_eq!(requested_width(Some("foo=1&w=400")), Some(400));
        assert_eq!(requested_width(None), None);
        assert_eq!(requested_width(Some("")), None);
        assert_eq!(requested_width(Some("w=")), None);
        assert_eq!(requested_width(Some("w=abc")), None);
        assert_eq!(
            requested_width(Some("w=4096")),
            None,
            "an unlisted width must not become a cache entry"
        );
    }

    #[test]
    fn large_images_are_downscaled_once_and_then_reused() {
        let dir = thumb_dir("reuse");
        let source = dir.join("figure.png");
        write_png(&source, 2000, 1000);

        let (first, generated) = scaled(&dir, "job-1", &source, 800);
        assert!(generated, "a 2000px source must produce a thumbnail");
        assert_eq!(first, dir.join("thumbs").join("job-1_800.jpg"));
        assert_eq!(dimensions(&first), (800, 400));
        // Decode cost scales with pixels, not bytes, and that decode is what
        // made the history grid stutter.
        let (width, height) = dimensions(&first);
        assert!(
            u64::from(width) * u64::from(height) * 6 < 2000 * 1000,
            "the thumbnail must cut the decode down by at least 6x"
        );

        // The atomic write must not leave staging files behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.join("thumbs"))
            .expect("读缩略图目录")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".part"))
            .collect();
        assert!(leftovers.is_empty(), "staging files must not survive");

        // A second call is a cache hit, not a re-encode.
        let stamp = std::fs::metadata(&first)
            .expect("元数据")
            .modified()
            .expect("mtime");
        let (again, _) = scaled(&dir, "job-1", &source, 800);
        assert_eq!(again, first);
        assert_eq!(
            std::fs::metadata(&again)
                .expect("元数据")
                .modified()
                .expect("mtime"),
            stamp,
            "a cache hit must not rewrite the file"
        );
    }

    #[test]
    fn images_already_small_enough_are_served_untouched() {
        let dir = thumb_dir("small");
        let source = dir.join("small.png");
        write_png(&source, 320, 200);

        let (path, generated) = scaled(&dir, "job-2", &source, 800);
        assert_eq!(path, source, "no point re-encoding a 320px image");
        assert!(!generated, "the caller must keep the original content type");
    }

    #[test]
    fn undecodable_files_fall_back_to_the_original() {
        let dir = thumb_dir("garbage");
        let source = dir.join("notes.pdf");
        std::fs::write(&source, b"%PDF-1.7 not really").expect("写测试文件");

        let (path, generated) = scaled(&dir, "job-3", &source, 800);
        assert_eq!(path, source, "a failed thumbnail must not hide the file");
        assert!(!generated);
    }

    #[test]
    fn a_regenerated_source_invalidates_its_thumbnail() {
        let dir = thumb_dir("stale");
        let source = dir.join("figure.png");
        write_png(&source, 1600, 800);
        let (thumb, _) = scaled(&dir, "job-4", &source, 400);
        assert_eq!(dimensions(&thumb), (400, 200));

        // Same id and width, new content: rerunning a job overwrites its
        // output in place, so the cache has to follow the file, not the key.
        // The pause keeps the new mtime clearly newer than the thumbnail's on
        // every filesystem CI runs on, not just the fine-grained ones.
        std::thread::sleep(std::time::Duration::from_millis(50));
        write_png(&source, 1600, 1600);

        let (again, generated) = scaled(&dir, "job-4", &source, 400);
        assert!(generated);
        assert_eq!(again, thumb);
        assert_eq!(
            dimensions(&again),
            (400, 400),
            "a stale thumbnail must be rebuilt from the newer source"
        );
    }
}
