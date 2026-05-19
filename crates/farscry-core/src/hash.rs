use crate::types::StateId;
use image::DynamicImage;
use rustdct::DctPlanner;

pub fn phash_image(image: &DynamicImage) -> StateId {
    let small = image.resize_exact(32, 32, image::imageops::FilterType::Nearest);

    let gray = small.to_luma8();

    let mut pixels: Vec<f32> = gray.pixels().map(|p| p[0] as f32).collect();

    let dct = compute_2d_dct(&mut pixels, 32);

    pack_phash_bits(&dct)
}

pub fn phash_from_bgra(data: &[u8], w: u32, h: u32) -> StateId {
    let wu = w as usize;
    let hu = h as usize;
    let bpr = data.len().checked_div(hu).unwrap_or(wu * 4);

    let mut gray_bytes = vec![0u8; 1024];
    for row in 0..32usize {
        let src_y = ((row as f64 + 0.5) * hu as f64 / 32.0) as usize;
        let src_y = src_y.min(hu.saturating_sub(1));
        for col in 0..32usize {
            let src_x = ((col as f64 + 0.5) * wu as f64 / 32.0) as usize;
            let src_x = src_x.min(wu.saturating_sub(1));
            let px = src_y * bpr + src_x * 4;
            if px + 2 < data.len() {
                let b = data[px] as f32;
                let g = data[px + 1] as f32;
                let r = data[px + 2] as f32;
                gray_bytes[row * 32 + col] = (0.299 * r + 0.587 * g + 0.114 * b) as u8;
            }
        }
    }

    let luma = image::GrayImage::from_raw(32, 32, gray_bytes)
        .map(DynamicImage::ImageLuma8)
        .expect("32×32 GrayImage");
    phash_image(&luma)
}

fn compute_2d_dct(input: &mut [f32], size: usize) -> Vec<f32> {
    let mut planner = DctPlanner::new();
    let dct = planner.plan_dct2(size);
    let mut output = vec![0.0f32; size * size];

    let mut row_scratch = vec![0.0f32; size];

    for i in 0..size {
        for j in 0..size {
            row_scratch[j] = input[i * size + j];
        }
        dct.process_dct2(&mut row_scratch);
        for j in 0..size {
            output[i * size + j] = row_scratch[j];
        }
    }

    for j in 0..size {
        for i in 0..size {
            row_scratch[i] = output[i * size + j];
        }
        dct.process_dct2(&mut row_scratch);
        for i in 0..size {
            output[i * size + j] = row_scratch[i];
        }
    }

    output
}

fn pack_phash_bits(dct: &[f32]) -> StateId {
    let mut low_freq_block = Vec::with_capacity(64);
    for freq_row in 0..8 {
        for freq_col in 0..8 {
            low_freq_block.push(dct[freq_row * 32 + freq_col]);
        }
    }

    let ac_coefficients: Vec<f32> = low_freq_block.iter().skip(1).copied().collect();

    let mean: f32 = ac_coefficients.iter().sum::<f32>() / ac_coefficients.len() as f32;

    let mut bits: u64 = 0;
    for (i, &val) in ac_coefficients.iter().enumerate() {
        if val > mean {
            bits |= 1 << i;
        }
    }

    StateId::from_bits(bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phash_same_image() {
        let img = image::RgbImage::new(100, 100);
        let dynamic = DynamicImage::ImageRgb8(img);

        let hash1 = phash_image(&dynamic);
        let hash2 = phash_image(&dynamic);

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_phash_different_images() {
        let mut img1 = image::RgbImage::new(100, 100);
        for (x, y, pixel) in img1.enumerate_pixels_mut() {
            let val = if (x + y) % 2 == 0 { 0 } else { 255 };
            *pixel = image::Rgb([val, val, val]);
        }

        let mut img2 = image::RgbImage::new(100, 100);
        for (x, y, pixel) in img2.enumerate_pixels_mut() {
            let val = if (x * y) % 3 == 0 { 0 } else { 255 };
            *pixel = image::Rgb([val, val, val]);
        }

        let hash1 = phash_image(&DynamicImage::ImageRgb8(img1));
        let hash2 = phash_image(&DynamicImage::ImageRgb8(img2));

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_state_id_size() {
        assert_eq!(std::mem::size_of::<StateId>(), 8);
    }

    #[test]
    fn test_state_id_display() {
        let state_id = StateId::from_bits(0x123456789ABCDEF0);
        let display = format!("{}", state_id);
        assert!(display.starts_with("phash:"));
        assert_eq!(display.len(), 22);

        assert_eq!(display, "phash:123456789abcdef0");
    }

    #[test]
    fn test_state_id_bits_conversion() {
        let bits = 0x123456789ABCDEF0;
        let state_id = StateId::from_bits(bits);
        assert_eq!(state_id.to_bits(), bits);
    }

    #[test]
    fn test_phash_determinism() {
        let img = image::RgbImage::new(100, 100);
        let dynamic = DynamicImage::ImageRgb8(img);

        let mut hashes = Vec::new();
        for _ in 0..100 {
            hashes.push(phash_image(&dynamic));
        }

        let first = hashes[0];
        for hash in &hashes[1..] {
            assert_eq!(first, *hash);
        }
    }

    #[test]
    fn test_phash_perceptual_stability_1px_shift() {
        use image::{ImageBuffer, Rgb};

        let mut img = ImageBuffer::<Rgb<u8>, _>::new(200, 100);
        for (x, y, px) in img.enumerate_pixels_mut() {
            let r = ((x * 255) / 200) as u8;
            let g = ((y * 255) / 100) as u8;
            *px = Rgb([r, g, 128]);
        }
        let original = DynamicImage::ImageRgb8(img.clone());

        let mut shifted = ImageBuffer::<Rgb<u8>, _>::new(200, 100);
        for (x, y, px) in shifted.enumerate_pixels_mut() {
            if y == 99 {
                *px = Rgb([0, 0, 0]);
            } else {
                *px = *img.get_pixel(x, y + 1);
            }
        }
        let shifted_img = DynamicImage::ImageRgb8(shifted);

        let hash_orig = phash_image(&original);
        let hash_shifted = phash_image(&shifted_img);

        assert_eq!(
            hash_orig, hash_shifted);
    }

    #[test]
    fn test_phash_sensitivity_error_banner() {
        use image::{ImageBuffer, Rgb};

        let white = ImageBuffer::<Rgb<u8>, _>::from_pixel(200, 100, Rgb([255, 255, 255]));
        let base = DynamicImage::ImageRgb8(white);

        let mut with_banner = ImageBuffer::<Rgb<u8>, _>::from_pixel(200, 100, Rgb([255, 255, 255]));
        for (_, y, px) in with_banner.enumerate_pixels_mut() {
            if y >= 80 {
                *px = Rgb([220, 30, 30]);
            }
        }
        let modified = DynamicImage::ImageRgb8(with_banner);

        let hash_base = phash_image(&base);
        let hash_banner = phash_image(&modified);

        assert_ne!(
            hash_base, hash_banner);
    }
}
