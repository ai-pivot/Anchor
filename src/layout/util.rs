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

// ── 缓动函数（Easing functions）──
// 所有函数输入 t ∈ [0, 1]，输出 ∈ [0, 1]

/// Deceleration curve — ease-out cubic (current default for workspace switch)
#[inline(always)]
pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// Fast deceleration — ease-out exponential
#[inline(always)]
pub fn ease_out_expo(t: f32) -> f32 {
    if t == 0.0 { 0.0 } else { 1.0 - 2.0f32.powf(-10.0 * t) }
}

/// Smooth in-out — cubic
#[inline(always)]
pub fn ease_in_out_cubic(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Overshoot bounce — ease-out back
#[inline(always)]
pub fn ease_out_back(t: f32) -> f32 {
    const C1: f32 = 1.70158;
    const C3: f32 = C1 + 1.0;
    1.0 + C3 * (t - 1.0).powi(3) + C1 * (t - 1.0).powi(2)
}

/// Softer deceleration — ease-out quart
#[inline(always)]
pub fn ease_out_quart(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(4)
}

/// Quick deceleration with slight bounce — ease-out elastic
#[inline(always)]
pub fn ease_out_elastic(t: f32) -> f32 {
    if t == 0.0 { return 0.0; }
    if t == 1.0 { return 1.0; }
    const C4: f32 = (2.0 * std::f32::consts::PI) / 3.0;
    2.0f32.powf(-10.0 * t) * ((t * 10.0 - 0.75) * C4).sin() + 1.0
}

/// Smooth acceleration — ease-in cubic
#[inline(always)]
pub fn ease_in_cubic(t: f32) -> f32 {
    t * t * t
}
