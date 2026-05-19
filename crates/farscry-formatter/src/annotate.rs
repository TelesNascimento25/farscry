use farscry_core::VaspOutput;
use image::{DynamicImage, Rgba};
use imageproc::drawing::draw_hollow_rect_mut;
use imageproc::rect::Rect;

const NEON: &[Rgba<u8>] = &[
    Rgba([255, 0, 128, 255]),
    Rgba([0, 255, 80, 255]),
    Rgba([0, 220, 255, 255]),
    Rgba([255, 220, 0, 255]),
    Rgba([200, 0, 255, 255]),
    Rgba([255, 80, 0, 255]),
    Rgba([0, 255, 200, 255]),
    Rgba([255, 0, 255, 255]),
    Rgba([80, 255, 0, 255]),
    Rgba([255, 160, 0, 255]),
    Rgba([0, 128, 255, 255]),
    Rgba([255, 255, 0, 255]),
];

pub fn annotate_image(img: DynamicImage, output: &VaspOutput) -> DynamicImage {
    let mut rgba = img.to_rgba8();
    let img_w = rgba.width() as i32;
    let img_h = rgba.height() as i32;

    let black = Rgba([0u8, 0, 0, 255]);

    let mut draw_inward_box = |x0: i32, y0: i32, w: u32, h: u32, color: Rgba<u8>| {
        if w == 0 || h == 0 {
            return;
        }
        draw_hollow_rect_mut(&mut rgba, Rect::at(x0, y0).of_size(w, h), black);
        if w > 2 && h > 2 {
            draw_hollow_rect_mut(
                &mut rgba,
                Rect::at(x0 + 1, y0 + 1).of_size(w - 2, h - 2),
                color,
            );
        }
        if w > 4 && h > 4 {
            draw_hollow_rect_mut(
                &mut rgba,
                Rect::at(x0 + 2, y0 + 2).of_size(w - 4, h - 4),
                color,
            );
        }
    };

    for (i, element) in output.ui_tree.iter().enumerate() {
        if element.w < 1.0 || element.h < 1.0 {
            continue;
        }
        let color = NEON[i % NEON.len()];

        let x0 = ((element.cx - element.w / 2.0).floor() as i32).clamp(0, img_w - 1);
        let y0 = ((element.cy - element.h / 2.0).floor() as i32).clamp(0, img_h - 1);
        let x1 = ((element.cx + element.w / 2.0).ceil() as i32).clamp(0, img_w - 1);
        let y1 = ((element.cy + element.h / 2.0).ceil() as i32).clamp(0, img_h - 1);
        let w = (x1 - x0).max(0) as u32;
        let h = (y1 - y0).max(0) as u32;

        draw_inward_box(x0, y0, w, h, color);
    }

    DynamicImage::ImageRgba8(rgba)
}
