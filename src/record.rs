//! 内置屏幕录制 — copy_framebuffer → bounded channel → 后台线程写入 ffmpeg → MP4
//! Super+R 开始/停止录制

use std::io::Write;
use std::process::Command;
use std::sync::mpsc::{self, SyncSender};
use tracing::info;

/// 屏幕录制状态
pub struct RecordState {
    /// 是否正在录制
    pub recording: bool,
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
            width: 0,
            height: 0,
            fps: 10,
            frame_tx: None,
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

        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-y", "-f", "rawvideo", "-pix_fmt", "rgba",
            "-s", &format!("{}x{}", width, height),
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
        self.width = width;
        self.height = height;
        self.recording = true;
        info!("🔴 开始录制 → {} ({}x{} @ {}fps)", output_path, width, height, self.fps);
    }

    /// 写入一帧（非阻塞，channel 满就跳过）
    pub fn write_frame(&mut self, data: &[u8]) {
        if !self.recording { return; }
        if let Some(ref tx) = self.frame_tx {
            let _ = tx.try_send(data.to_vec());
        }
    }

    /// 停止录制
    pub fn stop(&mut self) {
        if !self.recording { return; }
        self.recording = false;
        self.frame_tx = None;
        info!("⏹ 录制已停止");
    }
}

impl Drop for RecordState {
    fn drop(&mut self) { self.stop(); }
}
