#!/bin/bash
# Titan 启动脚本 — 重启后自动运行 winit 版本并截图
# 需要: GDM 正在运行, X11 可用

set -e
cd .

export DISPLAY=:0
export XAUTHORITY=/run/user/128/gdm/Xauthority

# 先确认 X11 可用
echo "检查 X11..."
xdpyinfo > /dev/null 2>&1 || {
    echo "❌ X11 不可用, 等待 5 秒再试..."
    sleep 5
    xdpyinfo > /dev/null 2>&1 || { echo "❌ X11 仍然不可用"; exit 1; }
}
echo "✅ X11 可用"

# 运行 titan-winit（后台，5 秒后自动退出）
echo "启动 titan-winit..."
timeout 8 ./target/release/titan-winit &
TITAN_PID=$!

# 等待窗口出现
sleep 3

# 截图
echo "截图..."
import -window root /tmp/titan_screenshot.png 2>/dev/null || \
xdotool search --name "Smithay" windowactivate --sync 2>/dev/null && import -window root /tmp/titan_screenshot.png 2>/dev/null || \
ffmpeg -f x11grab -video_size 1280x800 -i :0 -vframes 1 /tmp/titan_screenshot.png 2>/dev/null || \
echo "⚠️ 截图失败"

# 等待 titan 退出
wait $TITAN_PID 2>/dev/null || true
echo "✅ 完成"

# 显示截图信息
ls -la /tmp/titan_screenshot.png 2>/dev/null || echo "❌ 无截图文件"
