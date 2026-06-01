//! 内置屏幕录制 — 每帧 copy_framebuffer → pipe → ffmpeg 编码为 MP4
//! Super+R 开始/停止录制

use std::io::Write;
use std::process::{Child, ChildStdin};
use tracing::info;

/// 屏幕录制状态
pub struct RecordState {
    /// 是否正在录制
    pub recording: bool,
    /// ffmpeg 子进程
    process: Option<Child>,
    /// ffmpeg stdin pipe（写入原始帧数据）
    pipe: Option<ChildStdin>,
    /// 帧宽
    width: u32,
    /// 帧高
    height: u32,
    /// 帧率
    fps: u32,
}

impl RecordState {
    pub fn new() -> Self {
        Self {
            recording: false,
            process: None,
            pipe: None,
            width: 0,
            height: 0,
            fps: 30,
        }
    }

    /// 开始录制
    pub fn start(&mut self, width: u32, height: u32) {
        if self.recording { return; }

        let output_dir = std::env::var("HOME")
            .map(|h| format!("{}/Videos", h))
            .unwrap_or_else(|_| "/tmp".to_string());
        let _ = std::fs::create_dir_all(&output_dir);

        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let output_path = format!("{}/anchor_record_{}.mp4", output_dir, timestamp);

        // ffmpeg: raw rgba → h264 mp4
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.args([
            "-y",
            "-f", "rawvideo",
            "-pix_fmt", "rgba",
            "-s", &format!("{}x{}", width, height),
            "-r", &format!("{}", self.fps),
            "-i", "-",
            "-c:v", "libx264",
            "-preset", "ultrafast",
            "-crf", "23",
            "-pix_fmt", "yuv420p",
            &output_path,
        ]);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());

        match cmd.spawn() {
            Ok(mut child) => {
                if let Some(pipe) = child.stdin.take() {
                    self.pipe = Some(pipe);
                    self.process = Some(child);
                    self.width = width;
                    self.height = height;
                    self.recording = true;
                    info!("🔴 开始录制 → {} ({}x{} @ {}fps)", output_path, width, height, self.fps);
                } else {
                    let _ = child.kill();
                    info!("🔴 录制失败: 无法打开 ffmpeg stdin");
                }
            }
            Err(e) => {
                info!("🔴 录制失败: 无法启动 ffmpeg: {}", e);
            }
        }
    }

    /// 写入一帧原始 RGBA 数据
    pub fn write_frame(&mut self, data: &[u8]) {
        if !self.recording { return; }
        if let Some(ref mut pipe) = self.pipe {
            if pipe.write_all(data).is_err() {
                info!("🔴 录制管道断开，停止录制");
                self.stop();
            }
        }
    }

    /// 停止录制
    pub fn stop(&mut self) {
        if !self.recording { return; }
        self.recording = false;
        // 关闭 pipe（ffmpeg 收到 EOF 后自动完成编码）
        self.pipe = None;
        if let Some(mut child) = self.process.take() {
            // 等待 ffmpeg 完成编码（最多 5 秒）
            let _ = child.wait();
        }
        info!("⏹ 录制已停止");
    }

    /// 获取录制状态信息（用于 headbar 指示器）
    pub fn status_text(&self) -> Option<String> {
        if self.recording {
            Some("● REC".to_string())
        } else {
            None
        }
    }
}

impl Drop for RecordState {
    fn drop(&mut self) {
        self.stop();
    }
}
