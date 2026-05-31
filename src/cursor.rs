//! XCursor theme loader — loads cursor images from X11 cursor themes
//! Searches: ~/.icons/<theme>/cursors/, /usr/share/icons/<theme>/cursors/
//! Falls back to built-in pixel cursor if no theme is configured or loading fails

use std::path::PathBuf;

/// Pre-rendered cursor pixel data: (width, height, hotspot_x, hotspot_y, pixels)
/// pixels is RGBA row-major, scaled to the given pixel_size
pub struct CursorImage {
    pub width: usize,
    pub height: usize,
    pub hotspot_x: usize,
    pub hotspot_y: usize,
    /// RGBA pixels (4 bytes per pixel)
    pub pixels: Vec<u8>,
}

impl CursorImage {
    /// Load left_ptr cursor from an X11 cursor theme
    pub fn load_from_theme(theme: &str, cursor_name: &str, pixel_size: usize) -> Option<Self> {
        let home = std::env::var("HOME").unwrap_or_default();
        let search_dirs = [
            format!("{}/.icons/{}/cursors", home, theme),
            format!("{}/.local/share/icons/{}/cursors", home, theme),
            format!("/usr/share/icons/{}/cursors", theme),
            format!("/usr/share/icons/{}/cursors", theme.to_lowercase()),
        ];

        for dir in &search_dirs {
            let path = PathBuf::from(dir).join(cursor_name);
            if path.exists() {
                if let Some(img) = parse_xcursor_file(&path, pixel_size) {
                    tracing::info!("🖱️  光标主题加载: {} ({})", cursor_name, path.display());
                    return Some(img);
                }
            }
        }
        None
    }

    /// Built-in fallback: 16x16 left_ptr arrow (white with black outline)
    /// Same as the CURSOR_MAP in main.rs but as RGBA pixel data
    pub fn builtin(pixel_size: usize) -> Self {
        const MAP: [[u8; 16]; 16] = [
            [2,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
            [2,2,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
            [2,1,2,0,0,0,0,0,0,0,0,0,0,0,0,0],
            [2,1,1,2,0,0,0,0,0,0,0,0,0,0,0,0],
            [2,1,1,1,2,0,0,0,0,0,0,0,0,0,0,0],
            [2,1,1,1,1,2,0,0,0,0,0,0,0,0,0,0],
            [2,1,1,1,1,1,2,0,0,0,0,0,0,0,0,0],
            [2,1,1,1,1,1,1,2,0,0,0,0,0,0,0,0],
            [2,1,1,1,1,1,1,1,2,0,0,0,0,0,0,0],
            [2,1,1,1,1,1,1,1,1,2,2,0,0,0,0,0],
            [2,1,1,1,1,1,1,2,1,2,1,2,0,0,0,0],
            [2,1,1,1,1,1,2,0,1,1,2,1,2,0,0,0],
            [2,1,1,2,1,2,0,0,0,1,1,2,0,0,0,0],
            [2,1,2,0,2,0,0,0,0,0,1,2,0,0,0,0],
            [2,2,0,0,0,0,0,0,0,0,1,1,2,0,0,0],
            [2,0,0,0,0,0,0,0,0,0,0,1,2,0,0,0],
        ];
        let size = 16 * pixel_size;
        let mut pixels = vec![0u8; size * size * 4];
        for (y, row) in MAP.iter().enumerate() {
            for (x, &v) in row.iter().enumerate() {
                let (r, g, b, a) = match v {
                    1 => (0, 0, 0, 255),       // black outline
                    2 => (255, 255, 255, 255),   // white fill
                    _ => (0, 0, 0, 0),           // transparent
                };
                // Fill pixel_size x pixel_size block
                for dy in 0..pixel_size {
                    for dx in 0..pixel_size {
                        let px = x * pixel_size + dx;
                        let py = y * pixel_size + dy;
                        let idx = (py * size + px) * 4;
                        pixels[idx] = r;
                        pixels[idx + 1] = g;
                        pixels[idx + 2] = b;
                        pixels[idx + 3] = a;
                    }
                }
            }
        }
        CursorImage { width: size, height: size, hotspot_x: 0, hotspot_y: 0, pixels }
    }

    /// Render the cursor into a Pixman framebuffer at (cx, cy)
    pub fn render(&self, f: &mut impl smithay::backend::renderer::Frame, cx: i32, cy: i32) {
        use smithay::backend::renderer::Color32F;
        use smithay::utils::{Physical, Point, Rectangle, Size};

        for y in 0..self.height {
            for x in 0..self.width {
                let idx = (y * self.width + x) * 4;
                let a = self.pixels[idx + 3];
                if a == 0 { continue; }
                let r = self.pixels[idx] as f32 / 255.0;
                let g = self.pixels[idx + 1] as f32 / 255.0;
                let b = self.pixels[idx + 2] as f32 / 255.0;
                let color = Color32F::new(r, g, b, 1.0);
                let _ = f.clear(color, &[Rectangle::new(
                    Point::new(cx + x as i32, cy + y as i32),
                    Size::new(1, 1),
                )]);
            }
        }
    }
}

/// Minimal XCursor file parser (version 1 only, picks the best size)
/// XCursor format: https://www.x.org/releases/X11R7.7/doc/xcursor/specs/Xcursor.html
fn parse_xcursor_file(path: &std::path::Path, target_size: usize) -> Option<CursorImage> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 16 { return None; }

    // Header: magic (4 bytes), header_size (4), version (4), ntoc (4)
    let magic = u32::from_le_bytes(data[0..4].try_into().ok()?);
    if magic != 0x72756358 { return None; } // "Xcur"
    let _header_size = u32::from_le_bytes(data[4..8].try_into().ok()?) as usize;
    let version = u32::from_le_bytes(data[8..12].try_into().ok()?);
    if version > 1 { return None; }
    let ntoc = u32::from_le_bytes(data[12..16].try_into().ok()?) as usize;

    // Read TOC entries: find the image closest to target_size
    let mut best_entry: Option<(usize, usize)> = None; // (offset, size_diff)
    for i in 0..ntoc {
        let off = 16 + i * 12;
        if off + 12 > data.len() { break; }
        let _type = u32::from_le_bytes(data[off..off+4].try_into().ok()?);
        let _subtype = u32::from_le_bytes(data[off+4..off+8].try_into().ok()?);
        let entry_offset = u32::from_le_bytes(data[off+8..off+12].try_into().ok()?) as usize;
        if _type != 0xFFFD0002 { continue; } // IMAGE type
        let diff = if _subtype as usize >= target_size {
            _subtype as usize - target_size
        } else {
            target_size - _subtype as usize
        };
        if best_entry.is_none() || diff < best_entry.unwrap().1 {
            best_entry = Some((entry_offset, diff));
        }
    }
    let entry_offset = best_entry?.0;

    // Parse image chunk
    if entry_offset + 36 > data.len() { return None; }
    let chunk_header = u32::from_le_bytes(data[entry_offset..entry_offset+4].try_into().ok()?);
    if chunk_header != 0xFFFD0002 { return None; }
    let chunk_size = u32::from_le_bytes(data[entry_offset+8..entry_offset+12].try_into().ok()?) as usize;
    let width = u32::from_le_bytes(data[entry_offset+12..entry_offset+16].try_into().ok()?) as usize;
    let height = u32::from_le_bytes(data[entry_offset+16..entry_offset+20].try_into().ok()?) as usize;
    let hotspot_x = u32::from_le_bytes(data[entry_offset+20..entry_offset+24].try_into().ok()?) as usize;
    let hotspot_y = u32::from_le_bytes(data[entry_offset+24..entry_offset+28].try_into().ok()?) as usize;
    let _delay = u32::from_le_bytes(data[entry_offset+28..entry_offset+32].try_into().ok()?);

    // Pixels start at entry_offset + 36 (8 bytes chunk header + 4 * 7 fields = 36? No.)
    // Actually: chunk_header(4) + subtype(4) + chunk_size(4) + width(4) + height(4) + xhot(4) + yhot(4) + delay(4) = 32
    // But the standard says: header is 4 (type) + 4 (subtype) + 4 (version?) ... 
    // Let me re-read: the chunk starts with: type(4), subtype(4), length(4), then the image fields
    // After the 3 header u32s: width(4), height(4), xhot(4), yhot(4), delay(4), then pixels
    // So pixels start at entry_offset + 12 + 20 = entry_offset + 32
    let pixel_data_offset = entry_offset + 32;
    let expected_len = width * height * 4;
    if pixel_data_offset + expected_len > data.len() { return None; }

    // Convert ARGB32 to RGBA
    let mut pixels = vec![0u8; expected_len];
    for i in 0..(width * height) {
        let src_off = pixel_data_offset + i * 4;
        let dst_off = i * 4;
        let a = data[src_off + 3];
        let r = data[src_off + 2];
        let g = data[src_off + 1];
        let b = data[src_off];
        // Pre-multiplied alpha: if a < 255, un-premultiply
        if a > 0 && a < 255 {
            pixels[dst_off] = (r as u16 * 255 / a as u16).min(255) as u8;
            pixels[dst_off + 1] = (g as u16 * 255 / a as u16).min(255) as u8;
            pixels[dst_off + 2] = (b as u16 * 255 / a as u16).min(255) as u8;
            pixels[dst_off + 3] = a;
        } else {
            pixels[dst_off] = r;
            pixels[dst_off + 1] = g;
            pixels[dst_off + 2] = b;
            pixels[dst_off + 3] = a;
        }
    }

    Some(CursorImage { width, height, hotspot_x, hotspot_y, pixels })
}
