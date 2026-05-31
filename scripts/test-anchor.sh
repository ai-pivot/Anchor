#!/bin/bash
# Anchor 自动化测试脚本
# 用法: ./scripts/test-anchor.sh [description]
set -e
cd "$(dirname "$0")/.."

export PATH="$HOME/.cargo/bin:$PATH"

DESCRIPTION="${1:-test}"
SCREENSHOT_DIR="/tmp/anchor-screenshots"
mkdir -p "$SCREENSHOT_DIR"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
RAW_FILE="$SCREENSHOT_DIR/dump-${TIMESTAMP}.raw"
PNG_FILE="$SCREENSHOT_DIR/dump-${TIMESTAMP}.png"
PYTHON_CONVERT="$SCREENSHOT_DIR/convert.py"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

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
with open(sys.argv[2], 'wb') as f: f.write(png)
print(f'PNG: {sw}x{sh}, {len(png)} bytes')
PYEOF

echo "=== Anchor 自动化测试: $DESCRIPTION ==="
echo "[1/5] 编译 Anchor..."
cargo build --release --bin anchor 2>&1 | tail -3

echo "[2/5] 重启 GDM..."
sudo systemctl restart gdm 2>&1 || true

echo "[3/5] 等待 Anchor 启动..."
WAITED=0; MAX_WAIT=30; sleep 3
while [ $WAITED -lt $MAX_WAIT ]; do
    if pgrep -x anchor > /dev/null 2>&1; then echo "  Anchor 已启动 (${WAITED}s)"; break; fi
    sleep 1; WAITED=$((WAITED + 1))
done
if ! pgrep -x anchor > /dev/null 2>&1; then
    echo "❌ Anchor 未启动"; exit 1
fi
sleep 5

echo "[4/5] DRM 截图..."
sudo "$SCRIPT_DIR/drm-dump-fb" /dev/dri/card1 "$RAW_FILE" 2>&1
sudo chown $(id -u):$(id -g) "$RAW_FILE"
python3 "$PYTHON_CONVERT" "$RAW_FILE" "$PNG_FILE"
rm -f "$RAW_FILE"

echo "[5/5] 完成! 截图: $PNG_FILE"
echo "$PNG_FILE"
