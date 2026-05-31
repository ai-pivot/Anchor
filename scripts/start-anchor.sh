#!/bin/bash
# Anchor 合成器启动脚本
set -e
export TERMINAL="${TERMINAL:-foot}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
cd "$(dirname "$0")/.."

[ ! -f target/release/anchor ] && cargo build --release

# NVIDIA GBM/EGL 关键配置
export GBM_BACKEND=nvidia-drm
export __GLX_VENDOR_LIBRARY_NAME=nvidia
export __EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/10_nvidia.json

echo "🚀 启动 Anchor..."
exec sudo -E target/release/anchor
