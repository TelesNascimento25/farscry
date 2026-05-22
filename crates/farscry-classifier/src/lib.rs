pub mod element;
pub mod screen;
pub mod spatial;

#[cfg(test)]
mod bench;

use farscry_core::ScreenType;
use farscry_core::TextRegion;
use farscry_core::{ElementClassifier, ScreenClassifier, UiElement};

pub struct Classifier;

impl ScreenClassifier for Classifier {
    fn classify(&self, elements: &[UiElement]) -> ScreenType {
        let screen_w = elements
            .iter()
            .map(|e| e.cx + e.w / 2.0)
            .fold(0.0_f32, f32::max);
        let screen_h = elements
            .iter()
            .map(|e| e.cy + e.h / 2.0)
            .fold(0.0_f32, f32::max);
        let regions: Vec<TextRegion> = elements
            .iter()
            .map(|e| TextRegion {
                text: e.text.clone(),
                cx: e.cx,
                cy: e.cy,
                w: e.w,
                h: e.h,
            })
            .collect();
        screen::detect_screen_type_with_dims(&regions, screen_w, screen_h)
    }
}

impl ElementClassifier for Classifier {
    fn classify(&self, ocr: &farscry_core::OcrOutput) -> Vec<UiElement> {
        let screen_type = screen::detect_screen_type_with_dims(
            &ocr.regions,
            ocr.width as f32,
            ocr.height as f32,
        );
        element::classify_elements(&ocr.regions, screen_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classifier_integration() {
        let classifier = Classifier;

        let ocr_output = farscry_core::OcrOutput {
            regions: vec![
                TextRegion {
                    text: "Username:".to_string(),
                    cx: 50.0,
                    cy: 100.0,
                    w: 80.0,
                    h: 20.0,
                },
                TextRegion {
                    text: "Password:".to_string(),
                    cx: 50.0,
                    cy: 150.0,
                    w: 80.0,
                    h: 20.0,
                },
            ],
            width: 800,
            height: 600,
        };

        let elements = farscry_core::ElementClassifier::classify(&classifier, &ocr_output);
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0].element_type, farscry_core::ElementType::Label);
        assert_eq!(elements[1].element_type, farscry_core::ElementType::Label);
    }
}
