//! Anchor Header Bar 协议实现 (anchor-header-bar-v1)
//!
//! 允许客户端在合成器的 SSD 装饰区域内渲染自己的 header bar 内容。
//! 客户端声明期望的 header bar 高度，合成器在窗口上方预留空间，
//! 客户端在 toplevel 表面的顶部区域渲染标题栏内容。
//!
//! 同时修改 xdg-decoration handler，允许客户端选择 CSD 模式。

use std::sync::Mutex;

use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::ToplevelSurface;

// ── 协议代码生成 ──
// 使用 wayland-scanner 的 proc macros 从 XML 生成 Rust 绑定

mod generated {
    #![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
    #![allow(non_upper_case_globals, non_snake_case, unused_imports)]
    #![allow(missing_docs, clippy::all)]

    pub mod server {
        use wayland_server;
        use wayland_server::protocol::*;

        pub mod __interfaces {
            use wayland_backend;
            use wayland_server::protocol::__interfaces::*;
            // 导入 xdg_toplevel 接口（协议 XML 引用了它）
            pub use wayland_protocols::xdg::shell::server::__interfaces::xdg_toplevel_interface;
            pub use wayland_protocols::xdg::shell::server::__interfaces::XDG_TOPLEVEL_INTERFACE;
            wayland_scanner::generate_interfaces!("protocols/anchor-header-bar-v1.xml");
        }
        use self::__interfaces::*;

        // xdg_toplevel 类型在生成代码中被 super::xdg_toplevel 引用
        pub use wayland_protocols::xdg::shell::server::xdg_toplevel;

        wayland_scanner::generate_server_code!("protocols/anchor-header-bar-v1.xml");
    }
}

pub use generated::server::anchor_header_bar_manager_v1;
pub use generated::server::anchor_header_bar_v1;

// ── Per-window header bar 数据 ──

/// 每个 toplevel 窗口的 header bar 配置
/// 存储在 surface data_map 中
#[derive(Debug, Clone)]
pub struct HeaderBarData {
    /// 客户端请求的 header bar 高度（逻辑像素）
    /// 0 = 未设置（使用默认 SSD 装饰）
    pub height: i32,
    /// 合成器确认的实际高度
    pub confirmed_height: i32,
    /// 是否使用 CSD 模式（客户端自己绘制所有装饰）
    pub client_decoration: bool,
}

impl Default for HeaderBarData {
    fn default() -> Self {
        Self {
            height: 0,
            confirmed_height: 0,
            client_decoration: false,
        }
    }
}

// ── 辅助函数 ──

/// 读取某个 toplevel 的 header bar 高度
/// 返回 (header_bar_height, is_csd)
pub fn get_header_bar_info(tl: &ToplevelSurface) -> (i32, bool) {
    with_states(tl.wl_surface(), |states| {
        states
            .data_map
            .get::<Mutex<HeaderBarData>>()
            .and_then(|d| d.lock().ok())
            .map(|d| (d.confirmed_height, d.client_decoration))
            .unwrap_or((0, false))
    })
}

/// 设置某个 toplevel 的 header bar 高度
pub fn set_header_bar_height(tl: &ToplevelSurface, height: i32) {
    with_states(tl.wl_surface(), |states| {
        if let Some(data) = states.data_map.get::<Mutex<HeaderBarData>>() {
            if let Ok(mut d) = data.lock() {
                if d.confirmed_height == 0 {
                    d.height = height;
                    d.confirmed_height = height;
                }
            }
        }
    });
}

/// 标记某个 toplevel 使用 CSD
pub fn set_client_decoration(tl: &ToplevelSurface) {
    with_states(tl.wl_surface(), |states| {
        if let Some(data) = states.data_map.get::<Mutex<HeaderBarData>>() {
            if let Ok(mut d) = data.lock() {
                d.client_decoration = true;
            }
        }
    });
}

/// 初始化某个 toplevel 的 header bar 数据（如果尚未初始化）
pub fn ensure_header_bar_data(tl: &ToplevelSurface) {
    with_states(tl.wl_surface(), |states| {
        if states.data_map.get::<Mutex<HeaderBarData>>().is_none() {
            states
                .data_map
                .insert_if_missing_threadsafe(|| Mutex::new(HeaderBarData::default()));
        }
    });
}
