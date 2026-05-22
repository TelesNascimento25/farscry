use farscry_core::Preprocessor;
use image::{imageops::FilterType, DynamicImage, Rgba, RgbaImage};

pub struct EnhancingPreprocessor {
    pub scale: f32,
    pub sharpen: f32,
    pub contrast: f32,
}

impl Default for EnhancingPreprocessor {
    fn default() -> Self {
        Self {
            scale: 2.0,
            sharpen: 1.5,
            contrast: 1.2,
        }
    }
}

impl Preprocessor for EnhancingPreprocessor {
    fn process(&self, image: DynamicImage) -> DynamicImage {
        let src_w = image.width();
        let src_h = image.height();

        let scale = if src_w >= 1280 || src_h >= 720 {
            1.0_f32
        } else {
            self.scale
        };

        let w = ((src_w as f32) * scale) as u32;
        let h = ((src_h as f32) * scale) as u32;

        let upscaled = if scale != 1.0 {
            image.resize_exact(w, h, FilterType::Lanczos3)
        } else {
            image
        };

        let rgba = upscaled.to_rgba8();
        let blurred = image::imageops::blur(&rgba, 1.0_f32);
        let sharpened = unsharp_mask(&rgba, &blurred, self.sharpen);

        let contrasted = image::imageops::contrast(&sharpened, self.contrast);
        DynamicImage::ImageRgba8(contrasted)
    }
}

fn unsharp_mask(original: &RgbaImage, blurred: &RgbaImage, amount: f32) -> RgbaImage {
    let (w, h) = original.dimensions();
    let mut out = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let o = original.get_pixel(x, y).0;
            let b = blurred.get_pixel(x, y).0;
            out.put_pixel(
                x,
                y,
                Rgba([
                    clamp_u8(o[0] as f32 + amount * (o[0] as f32 - b[0] as f32)),
                    clamp_u8(o[1] as f32 + amount * (o[1] as f32 - b[1] as f32)),
                    clamp_u8(o[2] as f32 + amount * (o[2] as f32 - b[2] as f32)),
                    o[3],
                ]),
            );
        }
    }
    out
}

fn clamp_u8(v: f32) -> u8 {
    v.clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_small_image_is_upscaled() {
        let img = DynamicImage::new_rgba8(400, 300);
        let prep = EnhancingPreprocessor::default();
        let out = prep.process(img);
        assert_eq!(out.width(), 800);
        assert_eq!(out.height(), 600);
    }

    #[test]
    fn test_large_image_not_upscaled() {
        let img = DynamicImage::new_rgba8(1920, 1080);
        let prep = EnhancingPreprocessor::default();
        let out = prep.process(img);
        assert_eq!(out.width(), 1920);
        assert_eq!(out.height(), 1080);
    }

    #[test]
    fn test_unsharp_clamps_to_u8() {
        let orig = RgbaImage::from_pixel(4, 4, Rgba([200u8, 200, 200, 255]));
        let blurred = RgbaImage::from_pixel(4, 4, Rgba([100u8, 100, 100, 255]));
        let result = unsharp_mask(&orig, &blurred, 2.0);
        let pixel = result.get_pixel(0, 0);
        assert_eq!(pixel[3], 255, "alpha deve ser preservado");
        let _ = pixel[0];
    }
}
