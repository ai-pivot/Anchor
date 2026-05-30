#!/bin/bash
# Titan 自动化测试脚本
# 用法: ./scripts/test-titan.sh [description]
# 流程: build → kill titan → GDM 自动重启 → 等渲染 → DRM截图 → 分析

set -e
cd .

export PATH="$HOME/.cargo/bin:$PATH"

DESCRIPTION="${1:-test}"
SCREENSHOT_DIR="/tmp/titan-screenshots"
mkdir -p "$SCREENSHOT_DIR"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
RAW_FILE="$SCREENSHOT_DIR/dump-${TIMESTAMP}.raw"
PNG_FILE="$SCREENSHOT_DIR/dump-${TIMESTAMP}.png"
PYTHON_CONVERT="$SCREENSHOT_DIR/convert.py"

# 确保转换脚本存在
cat > "$PYTHON_CONVERT" << 'PYEOF'
import struct, zlib, array, sys

w, h = 2560, 1440
raw = open(sys.argv[1], 'rb').read()
sw, sh = 640, 360
pixels = array.array('B')
for y in range(sh):
    src_y = y * h // sh
    for x in range(sw):
        src_x = x * w // sw
        off = (src_y * w * 4) + src_x * 4
        b, g, r, a = raw[off], raw[off+1], raw[off+2], raw[off+3]
        pixels.extend([r, g, b])

def make_png(w, h, data):
    def chunk(ctype, data):
        c = ctype + data
        return struct.pack('>I', len(data)) + c + struct.pack('>I', zlib.crc32(c) & 0xffffffff)
    sig = b'\x89PNG\r\n\x1a\n'
    ihdr = chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0))
    raw_data = bytearray()
    for y in range(h):
        raw_data.append(0)
        raw_data.extend(data[y*w*3:(y+1)*w*3])
    idat = chunk(b'IDAT', zlib.compress(bytes(raw_data)))
    iend = chunk(b'IEND', b'')
    return sig + ihdr + idat + iend

png = make_png(sw, sh, bytes(pixels))
with open(sys.argv[2], 'wb') as f:
    f.write(png)
print(f'PNG: {sw}x{sh}, {len(png)} bytes')
PYEOF

echo "=== Titan 自动化测试: $DESCRIPTION ==="

# 1. 编译
echo "[1/5] 编译 Titan..."
cargo build --release --bin titan 2>&1 | tail -3

# 2. 重启 GDM 来杀掉 Titan 并触发自动登录
echo "[2/5] 重启 GDM (触发 Titan 自动登录)..."
sudo systemctl restart gdm 2>&1 || true

# 3. 等待 GDM 自动重启 Titan
echo "[3/5] 等待 GDM 自动启动 Titan..."
WAITED=0
MAX_WAIT=30
sleep 3  # GDM 重启后先等几秒
while [ $WAITED -lt $MAX_WAIT ]; do
    if pgrep -x titan > /dev/null 2>&1; then
        echo "  Titan 已启动 (等待 ${WAITED}s)"
        break
    fi
    sleep 1
    WAITED=$((WAITED + 1))
done

if ! pgrep -x titan > /dev/null 2>&1; then
    echo "❌ Titan 未能在 ${MAX_WAIT}s 内启动"
    sudo journalctl --since "1 min ago" --no-pager | grep -i "titan\|gdm" | tail -10
    exit 1
fi

# 等 Titan 渲染
sleep 5

# 4. DRM 截图
echo "[4/5] DRM 截图..."
sudo ./scripts/drm-dump-fb /dev/dri/card1 "$RAW_FILE" 2>&1
sudo chown user:user "$RAW_FILE"
python3 "$PYTHON_CONVERT" "$RAW_FILE" "$PNG_FILE"
rm -f "$RAW_FILE"

# 5. 输出
echo "[5/5] 完成!"
echo "截图: $PNG_FILE"
echo "$PNG_FILE"
