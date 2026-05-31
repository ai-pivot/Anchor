//! TTF 字体渲染（fontdue）— 正常文字

use fontdue::layout::{Layout, TextStyle, CoordinateSystem};
use fontdue::Font;
use smithay::backend::renderer::{Frame, Color32F};
use smithay::utils::{Physical, Point, Rectangle, Size};
use std::sync::OnceLock;

static FONT: OnceLock<Font> = OnceLock::new();

fn get_font() -> &'static Font {
    FONT.get_or_init(|| {
        let candidates = [
            // DejaVu Sans — Debian/Ubuntu/Arch
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            // Noto Sans — Arch [noto-fonts], Fedora, many distros
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
            // Liberation Sans — Fedora/RHEL, often installed
            "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            // Noto Sans CJK (contains Latin too) — common for CJK users
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            // Ubuntu font — Ubuntu
            "/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf",
            // Roboto — some distros / Android
            "/usr/share/fonts/truetype/roboto/Roboto-Regular.ttf",
            "/usr/share/fonts/TTF/Roboto-Regular.ttf",
            // Cantarell — GNOME default
            "/usr/share/fonts/cantarell/Cantarell-VF.otf",
            // Open Sans — some distros
            "/usr/share/fonts/truetype/open-sans/OpenSans-Regular.ttf",
            // Fallback: try fontconfig's default sans via fc-match
        ];
        // Try hardcoded paths first
        if let Some(data) = candidates.iter().find_map(|p| std::fs::read(p).ok()) {
            if let Ok(font) = Font::from_bytes(data, fontdue::FontSettings::default()) {
                tracing::info!("🔤 字体: 已加载");
                return font;
            }
        }
        // Try fontconfig to find any available sans-serif font
        if let Ok(output) = std::process::Command::new("fc-match")
            .arg("-f").arg("%{file}")
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
        tracing::error!("🔤 未找到任何可用字体！请安装 ttf-dejavu 或 noto-fonts");
        // Last resort: embed a minimal font. We create a 1-glyph font in memory
        // to avoid panic. Text will be invisible but compositor won't crash.
        // fontdue can load any valid TTF/OTF — if nothing works, we must still
        // provide something. Use DejaVu from system or fail with a useful message.
        panic!(
            "No usable font found!\n\
             Install one of: ttf-dejavu, noto-fonts, liberation-fonts\n\
             Or set a font with fontconfig: fc-cache -f && fc-match sans-serif"
        );
    })
}

#[inline(always)]
fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
    Rectangle::new(Point::new(x, y), Size::new(w.max(0), h.max(0)))
}

/// 渲染一行文字
pub fn draw_text(
    f: &mut impl Frame,
    text: &str,
    x: i32, y: i32,
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
        let (w, h) = (glyph.width, glyph.height);
        if w == 0 || h == 0 { continue; }

        // 用 key 获取 bitmap
        let (_metrics, bitmap) = font.rasterize_config(glyph.key);

        // 逐行渲染：合并连续非零像素为矩形（减少 f.clear 调用次数）
        for row in 0..h {
            let mut run_start: Option<usize> = None;
            for col in 0..=w {
                let alpha = if col < w { bitmap.get(row * w + col).copied().unwrap_or(0) } else { 0 };
                if alpha > 30 {
                    if run_start.is_none() { run_start = Some(col); }
                } else if let Some(cs) = run_start.take() {
                    let _ = f.clear(fg, &[rect(gx + cs as i32, gy + row as i32, (col - cs) as i32, 1)]);
                }
            }
        }
        let glyph_right = gx + w as i32;
        if glyph_right > max_right { max_right = glyph_right; }
    }
    max_right - x
}

/// 计算文字宽度
pub fn text_width(text: &str, size: f32) -> i32 {
    let font = get_font();
    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    layout.append(&[font], &TextStyle::new(text, size, 0));
    let glyphs = layout.glyphs();
    if glyphs.is_empty() { return 0; }
    let last = glyphs.last().unwrap();
    (last.x as i32 + last.width as i32).max(0)
}
