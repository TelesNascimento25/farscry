use farscry_core::{ElementType, ScreenType, TextRegion, UiElement};

const BUTTON_WORDS: &[&str] = &[
    "Save", "Cancel", "Submit", "Delete", "OK", "Apply", "Next", "Back", "Continue", "Close",
];

fn is_high_confidence_button(text: &str) -> bool {
    BUTTON_WORDS
        .iter()
        .any(|word| text.eq_ignore_ascii_case(word))
}

fn is_likely_interactive(region: &TextRegion) -> bool {
    let text = region.text.trim();
    let aspect = if region.h > 0.0 { region.w / region.h } else { 999.0 };

    if text.ends_with(':') {
        return false;
    }
    if text.ends_with('\u{2026}') || text.ends_with("...") {
        return false;
    }

    !text.is_empty()
        && text.len() < 40
        && aspect < 6.0
        && region.w > 20.0
        && region.h > 10.0
}

pub fn classify_elements(regions: &[TextRegion], screen_type: ScreenType) -> Vec<UiElement> {
    match screen_type {
        ScreenType::Terminal => classify_terminal(regions),
        ScreenType::Config => classify_config(regions),
        ScreenType::Error => classify_error(regions),
        ScreenType::Conversation => classify_conversation(regions),
        ScreenType::Ui | ScreenType::Unknown => classify_ui(regions),
    }
}

fn classify_terminal(regions: &[TextRegion]) -> Vec<UiElement> {
    regions
        .iter()
        .map(|region| UiElement {
            text: region.text.clone(),
            element_type: ElementType::Label,
            cx: region.cx,
            cy: region.cy,
            w: region.w,
            h: region.h,
            enabled: None,
            value: None,
        })
        .collect()
}

fn classify_config(regions: &[TextRegion]) -> Vec<UiElement> {
    regions
        .iter()
        .map(|region| {
            let element_type = classify_config_element(region);
            UiElement {
                text: region.text.clone(),
                element_type,
                cx: region.cx,
                cy: region.cy,
                w: region.w,
                h: region.h,
                enabled: None,
                value: None,
            }
        })
        .collect()
}

fn classify_config_element(region: &TextRegion) -> ElementType {
    if region.text.ends_with(':') {
        return ElementType::Label;
    }

    let aspect_ratio = region.w / region.h.max(1.0);
    if aspect_ratio > 4.0 && region.w > 150.0 {
        return ElementType::Input;
    }

    if is_high_confidence_button(&region.text) {
        return ElementType::Button;
    }

    if region.text.len() < 20
        && region
            .text
            .chars()
            .all(|c| c.is_uppercase() || c.is_whitespace())
    {
        return ElementType::Heading;
    }

    if is_likely_interactive(region) {
        return ElementType::Button;
    }

    ElementType::Label
}

fn classify_error(regions: &[TextRegion]) -> Vec<UiElement> {
    regions
        .iter()
        .map(|region| {
            let element_type = if region.text.to_lowercase().contains("error") {
                ElementType::Error
            } else {
                ElementType::Label
            };

            UiElement {
                text: region.text.clone(),
                element_type,
                cx: region.cx,
                cy: region.cy,
                w: region.w,
                h: region.h,
                enabled: None,
                value: None,
            }
        })
        .collect()
}

fn classify_conversation(regions: &[TextRegion]) -> Vec<UiElement> {
    regions
        .iter()
        .map(|region| {
            let word_count = region.text.split_whitespace().count();
            let element_type = if (1..=3).contains(&word_count) {
                ElementType::Heading
            } else {
                ElementType::Label
            };

            UiElement {
                text: region.text.clone(),
                element_type,
                cx: region.cx,
                cy: region.cy,
                w: region.w,
                h: region.h,
                enabled: None,
                value: None,
            }
        })
        .collect()
}

fn classify_ui(regions: &[TextRegion]) -> Vec<UiElement> {
    regions
        .iter()
        .map(|region| {
            let element_type = classify_ui_element(region);
            UiElement {
                text: region.text.clone(),
                element_type,
                cx: region.cx,
                cy: region.cy,
                w: region.w,
                h: region.h,
                enabled: None,
                value: None,
            }
        })
        .collect()
}

fn classify_ui_element(region: &TextRegion) -> ElementType {
    if is_high_confidence_button(&region.text) {
        return ElementType::Button;
    }

    if region.text.ends_with(':') {
        return ElementType::Label;
    }

    if is_likely_interactive(region) {
        return ElementType::Button;
    }

    let aspect_ratio = region.w / region.h.max(1.0);
    if aspect_ratio > 3.0 && region.w > 100.0 {
        return ElementType::Input;
    }

    ElementType::Label
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_all_labels() {
        let regions = vec![
            TextRegion {
                text: "$ ls -la".to_string(),
                cx: 0.0,
                cy: 0.0,
                w: 100.0,
                h: 20.0,
            },
            TextRegion {
                text: "drwxr-xr-x".to_string(),
                cx: 0.0,
                cy: 30.0,
                w: 100.0,
                h: 20.0,
            },
        ];

        let elements = classify_terminal(&regions);
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0].element_type, ElementType::Label);
        assert_eq!(elements[1].element_type, ElementType::Label);
    }

    #[test]
    fn test_config_colon_label() {
        let region = TextRegion {
            text: "Username:".to_string(),
            cx: 0.0,
            cy: 0.0,
            w: 100.0,
            h: 20.0,
        };

        let element_type = classify_config_element(&region);
        assert_eq!(element_type, ElementType::Label);
    }

    #[test]
    fn test_config_wide_input() {
        let region = TextRegion {
            text: "user@example.com".to_string(),
            cx: 0.0,
            cy: 0.0,
            w: 400.0,
            h: 30.0,
        };

        let element_type = classify_config_element(&region);
        assert_eq!(element_type, ElementType::Input);
    }

    #[test]
    fn test_config_button() {
        let region = TextRegion {
            text: "Save".to_string(),
            cx: 0.0,
            cy: 0.0,
            w: 80.0,
            h: 30.0,
        };

        let element_type = classify_config_element(&region);
        assert_eq!(element_type, ElementType::Button);
    }

    #[test]
    fn test_config_heading() {
        let region = TextRegion {
            text: "SETTINGS".to_string(),
            cx: 0.0,
            cy: 0.0,
            w: 100.0,
            h: 30.0,
        };

        let element_type = classify_config_element(&region);
        assert_eq!(element_type, ElementType::Heading);
    }

    #[test]
    fn test_error_detection() {
        let regions = vec![
            TextRegion {
                text: "TypeError: invalid operation".to_string(),
                cx: 0.0,
                cy: 0.0,
                w: 100.0,
                h: 20.0,
            },
            TextRegion {
                text: "at line 42".to_string(),
                cx: 0.0,
                cy: 30.0,
                w: 100.0,
                h: 20.0,
            },
        ];

        let elements = classify_error(&regions);
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0].element_type, ElementType::Error);
        assert_eq!(elements[1].element_type, ElementType::Label);
    }

    #[test]
    fn test_ui_open_folder_is_button() {
        let region = TextRegion {
            text: "Open Folder".to_string(),
            cx: 684.0,
            cy: 462.0,
            w: 120.0,
            h: 30.0,
        };
        assert_eq!(classify_ui_element(&region), ElementType::Button);
    }

    #[test]
    fn test_ui_font_color_is_button() {
        let region = TextRegion {
            text: "Font Color".to_string(),
            cx: 300.0,
            cy: 200.0,
            w: 100.0,
            h: 25.0,
        };
        assert_eq!(classify_ui_element(&region), ElementType::Button);
    }

    #[test]
    fn test_ui_long_text_is_label() {
        let region = TextRegion {
            text: "I am writing a word list for a dyslexic kid and this is very long".to_string(),
            cx: 400.0,
            cy: 300.0,
            w: 200.0,
            h: 80.0,
        };
        assert_eq!(classify_ui_element(&region), ElementType::Label);
    }

    #[test]
    fn test_ui_label_colon_not_button() {
        let region = TextRegion {
            text: "Username:".to_string(),
            cx: 100.0,
            cy: 100.0,
            w: 80.0,
            h: 20.0,
        };
        assert_eq!(classify_ui_element(&region), ElementType::Label);
    }

    #[test]
    fn test_ui_truncated_not_button() {
        let region = TextRegion {
            text: "This is a very long label that got…".to_string(),
            cx: 200.0,
            cy: 100.0,
            w: 90.0,
            h: 20.0,
        };
        assert_eq!(classify_ui_element(&region), ElementType::Label);
    }

    #[test]
    fn test_ui_wide_input_not_button() {
        let region = TextRegion {
            text: "user@example.com".to_string(),
            cx: 300.0,
            cy: 100.0,
            w: 350.0,
            h: 28.0,
        };
        assert_eq!(classify_ui_element(&region), ElementType::Input);
    }

    #[test]
    fn test_ui_high_aspect_not_button() {
        let region = TextRegion {
            text: "Some text".to_string(),
            cx: 200.0,
            cy: 100.0,
            w: 300.0,
            h: 18.0,
        };
        let aspect = 300.0_f32 / 18.0;
        assert!(aspect > 6.0, "aspect={aspect} deve ser > 6.0");
        assert_ne!(classify_ui_element(&region), ElementType::Button);
    }

    #[test]
    fn test_save_still_button_via_high_confidence() {
        let region = TextRegion {
            text: "Save".to_string(),
            cx: 600.0,
            cy: 400.0,
            w: 80.0,
            h: 30.0,
        };
        assert_eq!(classify_ui_element(&region), ElementType::Button);
    }

    #[test]
    fn test_config_font_effects_is_button() {
        let region = TextRegion {
            text: "Font Effects".to_string(),
            cx: 200.0,
            cy: 300.0,
            w: 110.0,
            h: 25.0,
        };
        assert_eq!(classify_config_element(&region), ElementType::Button);
    }

    #[test]
    fn test_conversation_speaker_heading() {
        let regions = vec![
            TextRegion {
                text: "Alice".to_string(),
                cx: 0.0,
                cy: 0.0,
                w: 50.0,
                h: 20.0,
            },
            TextRegion {
                text: "This is a longer message".to_string(),
                cx: 0.0,
                cy: 30.0,
                w: 200.0,
                h: 20.0,
            },
        ];

        let elements = classify_conversation(&regions);
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0].element_type, ElementType::Heading);
        assert_eq!(elements[1].element_type, ElementType::Label);
    }
}
