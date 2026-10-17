use core::ptr::write_volatile;

use elyon_abi::{BootInfo, FramebufferFormat};

const SCALE: usize = 3;

pub fn draw_boot_screen(info: &BootInfo, identity_limit: u64) -> bool {
    if info.framebuffer_base == 0
        || info.framebuffer_size == 0
        || info.framebuffer_base.saturating_add(info.framebuffer_size) > identity_limit
    {
        return false;
    }

    let width = info.framebuffer_width as usize;
    let height = info.framebuffer_height as usize;
    let stride = info.framebuffer_stride as usize;

    if width == 0 || height == 0 || stride < width {
        return false;
    }

    let bg = rgb(8, 14, 24, info.framebuffer_format);
    let accent = rgb(74, 144, 226, info.framebuffer_format);
    let white = rgb(238, 244, 252, info.framebuffer_format);

    fill(info, bg);

    let banner_h = height.min(72);
    for y in 0..banner_h {
        for x in 0..width {
            put_pixel(info, x, y, accent);
        }
    }

    draw_text(info, 28, 24, "ELYON OS", white);
    draw_text(info, 28, 100, "KERNEL 0.0.2", white);
    draw_text(info, 28, 145, "BOOT OK", accent);
    true
}

fn fill(info: &BootInfo, color: u32) {
    let pixels = (info.framebuffer_size as usize) / 4;
    let base = info.framebuffer_base as *mut u32;
    for i in 0..pixels {
        unsafe { write_volatile(base.add(i), color) };
    }
}

fn put_pixel(info: &BootInfo, x: usize, y: usize, color: u32) {
    if x >= info.framebuffer_width as usize || y >= info.framebuffer_height as usize {
        return;
    }
    let index = y
        .saturating_mul(info.framebuffer_stride as usize)
        .saturating_add(x);
    if index.saturating_mul(4) >= info.framebuffer_size as usize {
        return;
    }
    unsafe { write_volatile((info.framebuffer_base as *mut u32).add(index), color) };
}

fn draw_text(info: &BootInfo, mut x: usize, y: usize, text: &str, color: u32) {
    for ch in text.bytes() {
        if ch == b' ' {
            x += 6 * SCALE;
            continue;
        }
        draw_char(info, x, y, ch, color);
        x += 6 * SCALE;
    }
}

fn draw_char(info: &BootInfo, x: usize, y: usize, ch: u8, color: u32) {
    let glyph = glyph(ch);
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..5usize {
            if bits & (1 << (4 - col)) == 0 {
                continue;
            }
            for sy in 0..SCALE {
                for sx in 0..SCALE {
                    put_pixel(
                        info,
                        x + col * SCALE + sx,
                        y + row * SCALE + sy,
                        color,
                    );
                }
            }
        }
    }
}

fn glyph(ch: u8) -> [u8; 7] {
    match ch {
        b'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        b'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        b'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        b'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        b'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        b'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        b'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        b'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        b'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        b'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        b'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        b'0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        b'2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        b'.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
        _ => [0x1F, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],
    }
}

const fn rgb(r: u8, g: u8, b: u8, format: FramebufferFormat) -> u32 {
    match format {
        FramebufferFormat::Rgb => (r as u32) | ((g as u32) << 8) | ((b as u32) << 16),
        FramebufferFormat::Bgr => (b as u32) | ((g as u32) << 8) | ((r as u32) << 16),
        FramebufferFormat::None => 0,
    }
}
