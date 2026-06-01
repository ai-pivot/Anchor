//! 内置屏幕录制 — copy_framebuffer → bounded channel → 后台线程写入 ffmpeg → MP4
//! Super+R 全屏录制/停止，Super+Shift+R 区域录制

use std::io::Write;
use std::process::Command;
use std::sync::mpsc::{self, SyncSender};
use tracing::info;

/// 录制裁剪区域（全局坐标）
#[derive(Clone, Copy, Debug)]
pub struct RecordArea {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// 屏幕录制状态
pub struct RecordState {
    /// 是否正在录制
    pub recording: bool,
    /// 是否正在区域选择模式
    pub selecting: bool,
    /// 录制裁剪区域（None = 全屏）
    area: Option<RecordArea>,
    width: u32,
    height: u32,
    fps: u32,
    /// 帧数据发送通道（容量 2）
    frame_tx: Option<SyncSender<Vec<u8>>>,
}

impl RecordState {
    pub fn new() -> Self {
        Self {
            recording: false,
            selecting: false,
            area: None,
            width: 0,
            height: 0,
            fps: 20,
            frame_tx: None,
        }
    }

    /// 开始录制（全屏）
    pub fn start(&mut self, screen_w: u32, screen_h: u32) {
        self.start_with_area(screen_w, screen_h, None);
    }

    /// 开始录制（指定裁剪区域）
    pub fn start_with_area(&mut self, screen_w: u32, screen_h: u32, area: Option<RecordArea>) {
        if self.recording { return; }

        // 实际录制的尺寸
        let (rec_w, rec_h) = if let Some(a) = &area {
            (a.w, a.h)
        } else {
            (screen_w, screen_h)
        };

        if rec_w == 0 || rec_h == 0 { return; }

        let output_dir = std::env::var("HOME")
            .map(|h| format!("{}/Videos", h))
            .unwrap_or_else(|_| "/tmp".to_string());
        let _ = std::fs::create_dir_all(&output_dir);

        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let output_path = format!("{}/anchor_record_{}.mp4", output_dir, timestamp);

        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-y", "-f", "rawvideo", "-pix_fmt", "rgba",
            "-s", &format!("{}x{}", rec_w, rec_h),
            "-r", &format!("{}", self.fps),
            "-i", "-",
            "-c:v", "libx264", "-preset", "ultrafast", "-crf", "23",
            "-pix_fmt", "yuv420p",
            &output_path,
        ]);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => { info!("🔴 录制失败: {}", e); return; }
        };

        let pipe = match child.stdin.take() {
            Some(p) => p,
            None => { let _ = child.kill(); return; }
        };

        // 容量 2 的同步通道
        let (tx, rx): (SyncSender<Vec<u8>>, _) = mpsc::sync_channel(2);

        let _ = std::thread::Builder::new()
            .name("anchor-recorder".into())
            .spawn(move || {
                let mut pipe = pipe;
                while let Ok(frame) = rx.recv() {
                    if pipe.write_all(&frame).is_err() { break; }
                }
                drop(pipe);
                let _ = child.wait();
            });

        self.frame_tx = Some(tx);
        self.width = screen_w;
        self.height = screen_h;
        self.area = area;
        self.recording = true;
        if let Some(a) = &self.area {
            info!("🔴 开始区域录制 → {} ({}x{} @ {}fps)", output_path, a.w, a.h, self.fps);
        } else {
            info!("🔴 开始全屏录制 → {} ({}x{} @ {}fps)", output_path, screen_w, screen_h, self.fps);
        }
    }

    /// 裁剪并写入一帧（非阻塞，channel 满就跳过）
    pub fn write_frame(&mut self, full_pixels: &[u8], full_w: u32, full_h: u32) {
        if !self.recording { return; }
        if let Some(ref tx) = self.frame_tx {
            if let Some(a) = &self.area {
                // 区域录制：裁剪
                let row_len = full_w as usize * 4;
                let mut frame = vec![0u8; (a.w * a.h * 4) as usize];
                let dst_row = a.w as usize * 4;
                for row in 0..a.h as usize {
                    let src_start = ((a.y as usize + row) * row_len) + (a.x as usize * 4);
                    let src_end = src_start + dst_row;
                    let dst_start = row * dst_row;
                    if src_end <= full_pixels.len() && dst_start + dst_row <= frame.len() {
                        frame[dst_start..dst_start + dst_row]
                            .copy_from_slice(&full_pixels[src_start..src_end]);
                    }
                }
                let _ = tx.try_send(frame);
            } else {
                // 全屏录制：直接发送
                let _ = tx.try_send(full_pixels.to_vec());
            }
        }
    }

    /// 停止录制
    pub fn stop(&mut self) {
        if !self.recording { return; }
        self.recording = false;
        self.area = None;
        self.frame_tx = None;
        info!("⏹ 录制已停止");
    }
}

impl Drop for RecordState {
    fn drop(&mut self) { self.stop(); }
}
