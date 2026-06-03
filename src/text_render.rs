//! TTF 字体渲染（fontdue）— 正常文字

use fontdue::layout::{CoordinateSystem, GlyphRasterConfig, Layout, TextStyle};
use fontdue::Font;
use smithay::backend::renderer::{Color32F, Frame};
use smithay::utils::{Physical, Point, Rectangle, Size};
use std::collections::HashMap;
use std::sync::OnceLock;

static FONT: OnceLock<Font> = OnceLock::new();

// ---------------------------------------------------------------------------
// Glyph bitmap 缓存 — 避免每帧重复 CPU 光栅化
// ---------------------------------------------------------------------------

struct CachedGlyph {
    width: usize,
    height: usize,
    bitmap: Vec<u8>,
}

struct GlyphCache {
    entries: HashMap<(char, u32), CachedGlyph>,
}

thread_local! {
    static GLYPH_CACHE: std::cell::RefCell<GlyphCache> = std::cell::RefCell::new(GlyphCache {
        entries: HashMap::new(),
    });
}

/// 获取 glyph bitmap（命中缓存时零 CPU 光栅化）
fn cached_rasterize(font: &Font, ch: char, size: f32, key: GlyphRasterConfig) -> (usize, usize, Vec<u8>) {
    GLYPH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let cache_key = (ch, size.to_bits());
        let entry = cache.entries.entry(cache_key);
        match entry {
            std::collections::hash_map::Entry::Occupied(e) => {
                let g = e.get();
                (g.width, g.height, g.bitmap.clone())
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                let (metrics, bitmap) = font.rasterize_config(key);
                let w = metrics.width;
                let h = metrics.height;
                e.insert(CachedGlyph { width: w, height: h, bitmap: bitmap.clone() });
                // 缓存膨胀保护：超过 2000 条目时清空
                if cache.entries.len() > 2000 {
                    cache.entries.clear();
                }
                (w, h, bitmap)
            }
        }
    })
}

fn get_font() -> &'static Font {
    FONT.get_or_init(|| {
        // Prefer CJK font — covers Latin + CJK characters
        let cjk_candidates = [
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        ];
        for path in &cjk_candidates {
            if let Ok(data) = std::fs::read(path) {
                // TTC files need collection_index = 0
                if let Ok(font) = Font::from_bytes(
                    data,
                    fontdue::FontSettings {
                        collection_index: 0,
                        scale: 40.0,
                        load_substitutions: false,
                    },
                ) {
                    tracing::info!("🔤 字体(CJK): {}", path);
                    return font;
                }
            }
        }

        // Fallback: Latin-only fonts
        let latin_candidates = [
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf",
            "/usr/share/fonts/truetype/roboto/Roboto-Regular.ttf",
            "/usr/share/fonts/TTF/Roboto-Regular.ttf",
            "/usr/share/fonts/cantarell/Cantarell-VF.otf",
            "/usr/share/fonts/truetype/open-sans/OpenSans-Regular.ttf",
        ];
        for path in &latin_candidates {
            if let Ok(data) = std::fs::read(path) {
                if let Ok(font) = Font::from_bytes(data, fontdue::FontSettings::default()) {
                    tracing::info!("🔤 字体(Latin): {}", path);
                    return font;
                }
            }
        }

        // Fallback: fontconfig
        if let Ok(output) = std::process::Command::new("fc-match")
            .arg("-f")
            .arg("%{file}")
            .arg("sans-serif")
            .output()
        {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    if let Ok(data) = std::fs::read(&path) {
                        if let Ok(font) = Font::from_bytes(data, fontdue::FontSettings::default()) {
                            tracing::info!("🔤 字体: fc-match → {}", path);
                            return font;
                        }
                    }
                }
            }
        }
        panic!(
            "No usable font found!\n\
             Install: noto-fonts-cjk (recommended), or ttf-dejavu / noto-fonts"
        );
    })
}

#[inline(always)]
fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
    Rectangle::new(Point::new(x, y), Size::new(w.max(0), h.max(0)))
}

/// 渲染一行文字（批次优化：每个字形收集所有行矩形，一次 f.clear 调用）
pub fn draw_text(
    f: &mut impl Frame,
    text: &str,
    x: i32,
    y: i32,
    size: f32,
    color: (f32, f32, f32),
) -> i32 {
    let font = get_font();
    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    layout.append(&[font], &TextStyle::new(text, size, 0));

    let fg = Color32F::new(color.0, color.1, color.2, 1.0);
    let mut max_right = x;

    for glyph in layout.glyphs() {
        let gx = x + glyph.x as i32;
        let gy = y + glyph.y as i32;
        if glyph.width == 0 || glyph.height == 0 {
            continue;
        }

        let (w, h, bitmap) = cached_rasterize(font, glyph.parent, size, glyph.key);

        // Collect all run rectangles for this glyph, then issue one f.clear
        // 大部分 glyph 的非透明行数不超过 32，预分配避免反复扩容
        let mut glyph_rects: Vec<Rectangle<i32, Physical>> = Vec::with_capacity(h.min(32));
        for row in 0..h {
            let mut run_start: Option<usize> = None;
            for col in 0..=w {
                let alpha = if col < w {
                    bitmap.get(row * w + col).copied().unwrap_or(0)
                } else {
                    0
                };
                if alpha > 30 {
                    if run_start.is_none() {
                        run_start = Some(col);
                    }
                } else if let Some(cs) = run_start.take() {
                    glyph_rects.push(rect(gx + cs as i32, gy + row as i32, (col - cs) as i32, 1));
                }
            }
        }
        if !glyph_rects.is_empty() {
            let _ = f.clear(fg, &glyph_rects);
        }
        let glyph_right = gx + w as i32;
        if glyph_right > max_right {
            max_right = glyph_right;
        }
    }
    max_right - x
}

/// 计算文字宽度
pub fn text_width(text: &str, size: f32) -> i32 {
    let font = get_font();
    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    layout.append(&[font], &TextStyle::new(text, size, 0));
    let glyphs = layout.glyphs();
    if glyphs.is_empty() {
        return 0;
    }
    let last = glyphs.last().unwrap();
    (last.x as i32 + last.width as i32).max(0)
}
