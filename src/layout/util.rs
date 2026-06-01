//! 布局工具函数 + 排版常量
//!
//! 提供跨子模块共享的：
//! - 颜色辅助函数 (`opaque`, `color_hex`)
//! - 矩形构造辅助函数 (`rect`)
//! - 间距/字号常量 (`S1..S6`, `LOGO_SIZE`, ...)

use crate::config::parse_color;
use smithay::{
    backend::renderer::Color32F,
    utils::{Physical, Point, Rectangle, Size},
};

#[inline(always)]
pub fn opaque(r: f32, g: f32, b: f32) -> Color32F {
    Color32F::new(r, g, b, 1.0)
}

#[inline(always)]
pub fn color_hex(hex: &str) -> Color32F {
    let (r, g, b) = parse_color(hex);
    opaque(r, g, b)
}

#[inline(always)]
pub fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
    Rectangle::new(Point::new(x, y), Size::new(w, h))
}

// 间距 (spacing) 常量
pub const S1: i32 = 4;
pub const S2: i32 = 8;
pub const S3: i32 = 12;
pub const S4: i32 = 16;
pub const S6: i32 = 24;

// 字体大小
pub const LOGO_SIZE: f32 = 20.0;
pub const WS_SIZE: f32 = 16.0;
pub const TITLE_SIZE: f32 = 16.0;
pub const CLOCK_SIZE: f32 = 18.0;
pub const DATE_SIZE: f32 = 14.0;
