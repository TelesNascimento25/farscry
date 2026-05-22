use anyhow::{Context, Result};
use farscry_core::{ClassifiedScreen, Pipeline, VaspOutput};
use image::GenericImageView;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

pub static PIPELINE: OnceLock<Arc<Pipeline>> = OnceLock::new();

pub fn resolve_models_dir() -> PathBuf {
    if let Ok(p) = std::env::var("FARSCRY_MODELS_DIR") {
        let p = PathBuf::from(p);
        if p.exists() {
            return p;
        }
    }

    if let Some(home) = dirs::home_dir() {
        let p = home.join(".farscry").join("models");
        if p.exists() {
            return p;
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        let p = exe.parent().unwrap_or(Path::new(".")).join("models");
        if p.exists() {
            return p;
        }
    }

    let dev = PathBuf::from("spike").join("models");
    if dev.exists() {
        return dev;
    }

    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".farscry")
        .join("models")
}

pub fn get_or_build_pipeline() -> Result<Arc<Pipeline>> {
    if let Some(p) = PIPELINE.get() {
        return Ok(Arc::clone(p));
    }

    let models_dir = resolve_models_dir();

    let ocr = farscry_ocr::build_ocr_engine(&models_dir).map_err(|e| {
        anyhow::anyhow!(
            "OCR engine init failed: {e}\n\
            Tip: run `farscry setup` or set FARSCRY_MODELS_DIR"
        )
    })?;

    let pipeline = Arc::new(Pipeline::new(
        Arc::new(crate::preprocess::EnhancingPreprocessor::default()),
        Arc::new(ocr),
        Arc::new(farscry_classifier::Classifier),
        Arc::new(farscry_classifier::Classifier),
        Arc::new(PHashStateHasher),
        Arc::new(DefaultVaspFormatter),
    ));

    let _ = PIPELINE.set(Arc::clone(&pipeline));
    Ok(pipeline)
}

pub fn process_image(path: &Path, max_size: u64) -> Result<VaspOutput> {
    validate_image(path, max_size)?;

    let pipeline = get_or_build_pipeline()?;
    let img =
        image::open(path).with_context(|| format!("cannot open image: {}", path.display()))?;

    pipeline
        .process(img)
        .map_err(|e| anyhow::anyhow!("pipeline failed: {e}"))
}

pub fn validate_image(path: &Path, max_size: u64) -> Result<()> {
    if !path.is_file() {
        anyhow::bail!("file not found: {}", path.display());
    }

    let metadata = std::fs::metadata(path)?;
    let file_size = metadata.len();
    if file_size > max_size {
        anyhow::bail!(
            "file too large: {} bytes (max: {} bytes)",
            file_size,
            max_size
        );
    }

    let mut file = std::fs::File::open(path)?;
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)?;

    let is_png = magic.starts_with(&[0x89, 0x50, 0x4E, 0x47]);
    let is_jpg = magic.starts_with(&[0xFF, 0xD8, 0xFF]);
    let is_webp = magic.starts_with(&[0x52, 0x49, 0x46, 0x46]);
    let is_gif = magic.starts_with(&[0x47, 0x49, 0x46, 0x38]);
    let is_tiff = magic.starts_with(&[0x49, 0x49, 0x2A, 0x00])
        || magic.starts_with(&[0x4D, 0x4D, 0x00, 0x2A]);
    let is_pdf = magic.starts_with(b"%PDF");
    let is_svg = magic.starts_with(b"<svg") || magic.starts_with(b"<?xm");

    if is_pdf {
        anyhow::bail!("PDF not supported. Export as PNG first.");
    }
    if is_svg {
        anyhow::bail!("SVG not supported. Export as PNG first.");
    }
    if !is_png && !is_jpg && !is_webp && !is_gif && !is_tiff {
        anyhow::bail!("not an image file: {}", path.display());
    }

    let img = image::open(path)?;
    let (width, height) = img.dimensions();
    if width < 50 || height < 50 {
        anyhow::bail!("image too small: {}x{} (minimum: 50x50)", width, height);
    }

    Ok(())
}

struct IdentityPreprocessor;

impl farscry_core::Preprocessor for IdentityPreprocessor {
    fn process(&self, image: image::DynamicImage) -> image::DynamicImage {
        image
    }
}

struct PHashStateHasher;

impl farscry_core::StateHasher for PHashStateHasher {
    fn hash(&self, image: &image::DynamicImage) -> farscry_core::StateId {
        farscry_core::phash_image(image)
    }
}

struct DefaultVaspFormatter;

impl farscry_core::VaspFormatter for DefaultVaspFormatter {
    fn format(&self, screen: &ClassifiedScreen) -> VaspOutput {
        let ctx: String = screen
            .ui_tree
            .iter()
            .filter(|e| !e.text.is_empty())
            .map(|e| e.text.as_str())
            .collect::<Vec<_>>()
            .join(" \u{2022} ");
        let ctx = if ctx.len() > 120 {
            let boundary = ctx
                .char_indices()
                .map(|(i, _)| i)
                .take_while(|&i| i <= 120)
                .last()
                .unwrap_or(0);
            format!("{}\u{2026}", &ctx[..boundary])
        } else {
            ctx
        };

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
