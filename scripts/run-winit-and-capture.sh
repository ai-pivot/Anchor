#!/bin/bash
# Anchor winit 模式启动 + 截图
set -e
cd "$(dirname "$0")/.."

export DISPLAY="${DISPLAY:-:0}"
export XAUTHORITY="${XAUTHORITY:-/run/user/128/gdm/Xauthority}"

cargo build --release --bin anchor --features winit 2>&1 | tail -3
timeout 10 target/release/anchor 2>&1 &
PID=$!
sleep 5
kill $PID 2>/dev/null || true
