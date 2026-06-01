//! NVIDIA block-linear 像素格式转换
//!
//! NVIDIA GBM 创建的 scanout buffer 使用 block-linear 内存布局。
//! Pixman 渲染线性像素数据，需要转换为 block-linear 才能在显示器上正确显示。
//!
//! Block-linear 格式：
//! - GOB (Group of Bytes) = 64 bytes × 8 rows = 16 pixels × 8 lines (32bpp)
//! - Block = 1 GOB wide × N GOBs tall
//! - Block 内 GOB 按行堆叠
//! - Surface 按 block 栅格排列（左→右，上→下）

/// 将线性像素数据转换为 NVIDIA block-linear 布局
///
/// # 参数
/// - `src`: 线性布局的像素数据 (BGRA/XRGB, 4 bytes/pixel)
/// - `dst`: 目标 buffer（block-linear 布局）
/// - `width`: 图像宽度（像素）
/// - `height`: 图像高度（像素）
/// - `block_height_gobs`: 每个 block 的 GOB 数量（通常 16）
pub fn linear_to_block_linear(
    src: &[u8],
    dst: &mut [u8],
    width: usize,
    height: usize,
    block_height_gobs: usize,
) {
    let bpp = 4; // bytes per pixel
    let gob_w = 64 / bpp; // 16 pixels per GOB row
    let gob_h = 8; // 8 lines per GOB
    let gob_bytes = 512; // 64 * 8

    let num_gob_cols = (width + gob_w - 1) / gob_w;
    let block_size = block_height_gobs * gob_bytes;

    for y in 0..height {
        let gob_y = y / gob_h;
        let block_row = gob_y / block_height_gobs;
        let gob_in_block = gob_y % block_height_gobs;
        let y_in_gob = y % gob_h;

        for x in 0..width {
            let gob_x = x / gob_w;
            let x_in_gob = x % gob_w;

            // Block-linear 地址计算
            let bl_offset = (block_row * num_gob_cols + gob_x) * block_size
                + gob_in_block * gob_bytes
                + y_in_gob * 64
                + x_in_gob * bpp;

            // 线性地址
            let lin_offset = y * width * bpp + x * bpp;

            if bl_offset + bpp <= dst.len() && lin_offset + bpp <= src.len() {
                dst[bl_offset..bl_offset + bpp].copy_from_slice(&src[lin_offset..lin_offset + bpp]);
            }
        }
    }
}

/// 对 buffer 做原地线性→block-linear转换
pub fn convert_in_place(data: &mut [u8], width: usize, height: usize, block_height_gobs: usize) {
    let total = width * height * 4;
    if data.len() < total {
        return;
    }
    let mut temp = vec![0u8; total];
    temp[..total].copy_from_slice(&data[..total]);
    linear_to_block_linear(&temp, data, width, height, block_height_gobs);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_small_block_linear() {
        // 16x8 surface, 1 GOB, block_height = 1
        let w = 16;
        let h = 8;
        let bh = 1;
        let mut src = vec![0u8; w * h * 4];
        let mut dst = vec![0u8; w * h * 4];

        // Write a pattern: each pixel has unique color
        for y in 0..h {
            for x in 0..w {
                let off = (y * w + x) * 4;
                src[off] = x as u8;
                src[off + 1] = y as u8;
                src[off + 2] = (x + y) as u8;
                src[off + 3] = 0xFF;
            }
        }

        linear_to_block_linear(&src, &mut dst, w, h, bh);

        // For 1 GOB block-linear, the layout should be identical to linear
        // because block_col=0, block_row=0, gob_in_block=0 for all pixels
        assert_eq!(src, dst);
    }

    #[test]
    fn test_two_gobs_wide() {
        // 32x8 surface, 2 GOB columns, block_height = 1
        let w = 32;
        let h = 8;
        let bh = 1;
        let mut src = vec![0u8; w * h * 4];
        let mut dst = vec![0u8; w * h * 4];

        for y in 0..h {
            for x in 0..w {
                let off = (y * w + x) * 4;
                src[off] = x as u8;
                src[off + 1] = y as u8;
                src[off + 2] = 0xFF;
                src[off + 3] = 0xFF;
            }
        }

        linear_to_block_linear(&src, &mut dst, w, h, bh);

        // GOB 0 (x=0..15) should be at offset 0..512
        // GOB 1 (x=16..31) should be at offset 512..1024
        // Pixel (0,0) → bl_offset = 0*512 + 0*64 + 0*4 = 0
        // Pixel (16,0) → bl_offset = 1*512 + 0*64 + 0*4 = 512
        // Pixel (0,4) → bl_offset = 0*512 + 4*64 + 0*4 = 256
        assert_eq!(dst[0], 0); // pixel (0,0).r = 0
        assert_eq!(dst[512], 16); // pixel (16,0).r = 16
        assert_eq!(dst[256], 0); // pixel (0,4).r = 0
        assert_eq!(dst[257], 4); // pixel (0,4).g = 4
    }
}
