use crate::error::FarscryError;
use crate::types::{OcrOutput, ScreenType, StateId, UiElement, VaspDelta, VaspOutput};
use image::DynamicImage;

pub trait Preprocessor: Send + Sync + 'static {
    fn process(&self, image: DynamicImage) -> DynamicImage;
}

pub trait OcrEngine: Send + Sync + 'static {
    fn extract(&self, image: &DynamicImage) -> Result<OcrOutput, FarscryError>;
}

pub trait ElementClassifier: Send + Sync + 'static {
    fn classify(&self, ocr: &OcrOutput) -> Vec<UiElement>;
}

pub trait ScreenClassifier: Send + Sync + 'static {
    fn classify(&self, elements: &[UiElement]) -> ScreenType;
}

pub trait StateHasher: Send + Sync + 'static {
    fn hash(&self, image: &DynamicImage) -> StateId;
}

pub trait VaspFormatter: Send + Sync + 'static {
    fn format(&self, screen: &crate::types::ClassifiedScreen) -> VaspOutput;
}

pub trait DiffEngine: Send + Sync + 'static {
    fn diff(
        &self,
        before: &VaspOutput,
        after: &VaspOutput,
        before_dims: Option<(u32, u32)>,
        after_dims: Option<(u32, u32)>,
    ) -> VaspDelta;
}
