use farscry_core::{FarscryError, OcrEngine, OcrOutput, TextRegion};
use image::DynamicImage;
use oar_ocr::core::config::{OrtGraphOptimizationLevel, OrtSessionConfig};
use oar_ocr::domain::TextDetectionConfig;
use oar_ocr::prelude::*;
use oar_ocr::processors::LimitType;
use std::path::Path;
#[cfg(test)]
use std::time::Duration;
use std::time::Instant;

pub struct OrtOcrEngine {
    ocr: OAROCR,
}

impl OrtOcrEngine {
    pub fn from_models_dir(models_dir: &Path) -> Result<Self, FarscryError> {
        Self::new(models_dir)
    }

    pub fn new(models_dir: &Path) -> Result<Self, FarscryError> {
        crate::verify::verify_models(models_dir)?;

        let logical = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let physical = if cfg!(target_arch = "x86_64") {
            (logical / 2).max(1)
        } else {
            logical
        };

        let ort_config = OrtSessionConfig::new()
            .with_intra_threads(physical)
            .with_inter_threads(1)
            .with_optimization_level(OrtGraphOptimizationLevel::Level2);

        let det_config = TextDetectionConfig {
            limit_side_len: Some(640),
            limit_type: Some(LimitType::Max),
            ..TextDetectionConfig::default()
        };

        let ocr = OAROCRBuilder::new(
            models_dir.join("pp-ocrv5_mobile_det.onnx"),
            models_dir.join("en_pp-ocrv5_mobile_rec.onnx"),
            models_dir.join("ppocrv5_en_dict.txt"),
        )
        .ort_session(ort_config)
        .text_detection_config(det_config)
        .region_batch_size(32)
        .build()
        .map_err(|e| FarscryError::OcrFailed(format!("Failed to build OCR engine: {e}")))?;

        Ok(Self { ocr })
    }

    fn run_pipeline(&self, image: &DynamicImage) -> Result<OcrOutput, FarscryError> {
        let rgb_image = to_rgb_image(image)?;
        let image_width = image.width();
        let image_height = image.height();

        let ocr_results = self
            .ocr
            .predict(vec![rgb_image])
            .map_err(|e| FarscryError::OcrFailed(format!("OCR prediction failed: {e}")))?;

        if ocr_results.is_empty() {
            return Err(FarscryError::OcrFailed(
                "No OCR results returned".to_string(),
            ));
        }

        let regions = extract_text_regions(&ocr_results[0].text_regions);

        Ok(OcrOutput {
            regions,
            width: image_width,
            height: image_height,
        })
    }
}

fn to_rgb_image(image: &DynamicImage) -> Result<image::RgbImage, FarscryError> {
    match image {
        DynamicImage::ImageRgb8(img) => Ok(img.clone()),
        DynamicImage::ImageRgba8(img) => {
            let mut rgb_image = image::RgbImage::new(img.width(), img.height());
            for (x, y, pixel) in img.enumerate_pixels() {
                rgb_image[(x, y)] = image::Rgb([pixel.0[0], pixel.0[1], pixel.0[2]]);
            }
            Ok(rgb_image)
        }
        DynamicImage::ImageLuma8(img) => Ok(image::RgbImage::from_fn(
            img.width(),
            img.height(),
            |x, y| {
                let luma = img.get_pixel(x, y).0[0];
                image::Rgb([luma, luma, luma])
            },
        )),
        DynamicImage::ImageRgb16(_) => {
            Err(FarscryError::OcrFailed("RGB16 not supported".to_string()))
        }
        DynamicImage::ImageRgba16(_) => {
            Err(FarscryError::OcrFailed("RGBA16 not supported".to_string()))
        }
        _ => Err(FarscryError::OcrFailed(
            "Unsupported image format".to_string(),
        )),
    }
}

fn extract_text_regions(raw_regions: &[oar_ocr::domain::TextRegion]) -> Vec<TextRegion> {
    raw_regions
        .iter()
        .filter_map(|text_region| {
            text_region.text_with_confidence().map(|(text, _conf)| {
                let points = &text_region.bounding_box.points;
                let (bbox_w, bbox_h) = if points.len() >= 4 {
                    let bbox_w = ((points[1].x - points[0].x).powi(2)
                        + (points[1].y - points[0].y).powi(2))
                    .sqrt()
                    .max(1.0);
                    let bbox_h = ((points[3].x - points[0].x).powi(2)
                        + (points[3].y - points[0].y).powi(2))
                    .sqrt()
                    .max(1.0);
                    (bbox_w, bbox_h)
                } else {
                    let min_x = points.iter().map(|p| p.x).fold(f32::MAX, f32::min);
                    let max_x = points.iter().map(|p| p.x).fold(f32::MIN, f32::max);
                    let min_y = points.iter().map(|p| p.y).fold(f32::MAX, f32::min);
                    let max_y = points.iter().map(|p| p.y).fold(f32::MIN, f32::max);
                    ((max_x - min_x).max(1.0), (max_y - min_y).max(1.0))
                };
                let center = text_region.bounding_box.center();
                (text.to_string(), center.x, center.y, bbox_w, bbox_h)
            })
        })
        .map(|(text, cx, cy, w, h)| TextRegion { text, cx, cy, w, h })
        .collect()
}

impl OcrEngine for OrtOcrEngine {
    fn extract(&self, image: &DynamicImage) -> Result<OcrOutput, FarscryError> {
        let start = Instant::now();
        let result = self.run_pipeline(image);
        let elapsed = start.elapsed();

        if elapsed.as_millis() > 400 {
            eprintln!(
                "[farscry] OCR inference took {}ms (target: <400ms cold, <320ms warm)",
                elapsed.as_millis()
            );
        }

        result
    }
}

#[cfg(all(test, feature = "integration-tests"))]
mod integration {
    use super::OrtOcrEngine;
    use farscry_core::{
        phash_image, ClassifiedScreen, Pipeline, Preprocessor, StateHasher, StateId, VaspFormatter,
        VaspOutput,
    };
    use image::DynamicImage;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    struct IdentityPreprocessor;
    impl Preprocessor for IdentityPreprocessor {
        fn process(&self, img: DynamicImage) -> DynamicImage {
            img
        }
    }

    struct PHashHasher;
    impl StateHasher for PHashHasher {
        fn hash(&self, img: &DynamicImage) -> StateId {
            phash_image(img)
        }
    }

    struct SimpleFormatter;
    impl VaspFormatter for SimpleFormatter {
        fn format(&self, screen: &ClassifiedScreen) -> VaspOutput {
            let ctx = screen
                .ui_tree
                .iter()
                .filter(|e| !e.text.is_empty())
                .map(|e| e.text.as_str())
                .collect::<Vec<_>>()
                .join(" • ");
            VaspOutput::new(
                screen.state_id,
                screen.screen_type,
                screen.confidence,
                &screen.lang,
                ctx,
                screen.ui_tree.clone(),
                vec![],
            )
        }
    }

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("could not resolve repo root")
            .to_path_buf()
    }

    fn build_pipeline() -> Pipeline {
        let models_dir = repo_root().join("spike").join("models");
        assert!(models_dir.exists(), models_dir.display());

        let ocr = OrtOcrEngine::new(&models_dir).expect("Failed to load OCR engine");
        Pipeline::new(
            Arc::new(IdentityPreprocessor),
            Arc::new(ocr),
            Arc::new(farscry_classifier::Classifier),
            Arc::new(farscry_classifier::Classifier),
            Arc::new(PHashHasher),
            Arc::new(SimpleFormatter),
        )
    }

    #[test]
    fn test_real_pipeline_extract() {
        let test_image_path = repo_root().join("spike").join("test.png");
        assert!(test_image_path.exists(), test_image_path.display());

        let pipeline = build_pipeline();

        let img = image::open(&test_image_path).expect("Failed to open spike/test.png");
        let result = pipeline.process(img).expect("Pipeline failed");

        assert!(
            result.state_id.to_string().starts_with("phash:"),
            result.state_id
        );
        assert_eq!(result.state_id.to_string().len(), 22);

        assert!(!result.agent_context.is_empty());

        assert!(result.ui_tree.len() >= 20, result.ui_tree.len());

        eprintln!(
            "[integration] PASS - {} UI elements, state_id={}, agent_context=\"{}\"",
            result.ui_tree.len(),
            result.state_id,
            result.agent_context,
        );
        for (i, elem) in result.ui_tree.iter().take(5).enumerate() {
            eprintln!(
                "  [{}] {:?} \"{}\" @ ({:.0},{:.0})",
                i, elem.element_type, elem.text, elem.cx, elem.cy
            );
        }
    }

    #[test]
    fn test_real_pipeline_diff() {
        let bench_dir = repo_root()
            .join("spike")
            .join("benchmark")
            .join("screenshots");
        let img1_path = bench_dir.join("01.png");
        let img2_path = bench_dir.join("02.png");

        if !img1_path.exists() || !img2_path.exists() {
            eprintln!("[integration] Skipping diff test: benchmark screenshots not found");
            return;
        }

        let pipeline = build_pipeline();

        let img1 = image::open(&img1_path).expect("Failed to open 01.png");
        let img2 = image::open(&img2_path).expect("Failed to open 02.png");

        let vasp1 = pipeline.process(img1).expect("Pipeline failed on img1");
        let vasp2 = pipeline.process(img2).expect("Pipeline failed on img2");

        assert_ne!(vasp1.state_id, vasp2.state_id);

        let diff_engine = farscry_diff::DiffEngineImpl;
        use farscry_core::DiffEngine;
        let delta = diff_engine.diff(&vasp1, &vasp2, None, None);

        assert!(delta.vasp_version == "1.0");
        eprintln!(
            "[integration] Diff PASS - context_similarity={:.3}, {} delta entries",
            delta.context_similarity,
            delta.entries.len()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_engine_creation_missing_models() {
        let temp_dir = env::temp_dir();
        let result = OrtOcrEngine::new(&temp_dir);

        assert!(result.is_err());
    }

    #[test]
    fn test_stub_pipeline() {
        let temp_dir = env::temp_dir();
        let result = OrtOcrEngine::new(&temp_dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_physical_cores_calculation() {
        let logical = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let physical = if cfg!(target_arch = "x86_64") {
            (logical / 2).max(1)
        } else {
            logical
        };

        assert!(physical <= logical);

        assert!(physical >= 1);
    }

    #[test]
    #[ignore]
    fn test_integration_with_actual_models() {
        let current_dir = env::current_dir().expect("Could not get current directory");

        let repo_root = current_dir
            .ancestors()
            .nth(2)
            .expect("Could not find repo root")
            .to_path_buf();

        let models_dir = repo_root.join("spike").join("models");
        let test_image = repo_root.join("spike").join("test.png");

        eprintln!("Current directory: {}", current_dir.display());
        eprintln!("Repo root: {}", repo_root.display());
        eprintln!("Models directory: {}", models_dir.display());
        eprintln!("Test image: {}", test_image.display());

        if !models_dir.exists() {
            panic!(
                "Integration test: models directory not found at {}",
                models_dir.display()
            );
        }

        if !test_image.exists() {
            panic!(
                "Integration test: test image not found at {}",
                test_image.display()
            );
        }

        eprintln!("Loading OCR engine...");
        let engine = OrtOcrEngine::new(&models_dir).expect("Failed to load OCR engine");
        eprintln!("OCR engine loaded successfully");

        eprintln!("Loading test image...");
        let image = image::open(&test_image).expect("Failed to load test image");
        eprintln!("Test image loaded: {}x{}", image.width(), image.height());

        eprintln!("Running OCR benchmark (10 iterations)...");
        let mut timings = Vec::new();

        for i in 0..10 {
            let start = Instant::now();
            let result = engine.extract(&image).expect("OCR extraction failed");
            let elapsed = start.elapsed();

            timings.push(elapsed);
            eprintln!(
                "Run {}: {}ms ({} regions)",
                i + 1,
                elapsed.as_millis(),
                result.regions.len()
            );

            if i == 0 {
                eprintln!(
                    "First run - OCR found {} text regions",
                    result.regions.len()
                );
                eprintln!("Image dimensions: {}x{}", result.width, result.height);

                for (j, region) in result.regions.iter().enumerate() {
                    eprintln!(
                        "Region {}: '{}' at ({:.1}, {:.1})",
                        j, region.text, region.cx, region.cy
                    );
                }
            }
        }

        let cold_time = timings[0];
        let warm_times: Vec<_> = timings.iter().skip(1).cloned().collect();
        let warm_avg = warm_times.iter().sum::<Duration>() / warm_times.len() as u32;
        let warm_min = warm_times.iter().min().unwrap();
        let warm_max = warm_times.iter().max().unwrap();

        eprintln!("\n=== Performance Summary ===");
        eprintln!("Cold run: {}ms", cold_time.as_millis());
        eprintln!("Warm average (runs 2-10): {}ms", warm_avg.as_millis());
        eprintln!("Warm min: {}ms", warm_min.as_millis());
        eprintln!("Warm max: {}ms", warm_max.as_millis());

        assert!(!timings[0].is_zero());
        assert!(warm_avg.as_millis() > 0);

        let cold_target = Duration::from_millis(400);
        let warm_target = Duration::from_millis(320);

        eprintln!("\n=== Target Comparison ===");
        eprintln!(
            "Cold target: <{}ms (actual: {}ms) {}",
            cold_target.as_millis(),
            cold_time.as_millis(),
            if cold_time < cold_target {
                " PASS"
            } else {
                "No FAIL"
            }
        );
        eprintln!(
            "Warm target: <{}ms (actual: {}ms) {}",
            warm_target.as_millis(),
            warm_avg.as_millis(),
            if warm_avg < warm_target {
                " PASS"
            } else {
                "No FAIL"
            }
        );
    }
}
