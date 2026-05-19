pub mod annotate;
pub mod context;
pub mod diff;
pub mod vasp;

use farscry_core::{VaspDelta, VaspOutput};

pub struct VaspFormatter;

impl VaspFormatter {
    pub fn format_vasp(
        output: &VaspOutput,
        source: &str,
        image_width: u32,
        image_height: u32,
    ) -> String {
        vasp::format_vasp(output, source, image_width, image_height)
    }

    pub fn format_vasp_with_options(
        output: &VaspOutput,
        source: &str,
        image_width: u32,
        image_height: u32,
        show_affordances: bool,
    ) -> String {
        vasp::format_vasp_with_options(output, source, image_width, image_height, show_affordances)
    }

    pub fn format_text_only(output: &VaspOutput) -> String {
        vasp::format_text_only(output)
    }

    pub fn format_json(output: &VaspOutput, pretty: bool) -> String {
        vasp::format_json(output, pretty)
    }

    pub fn format_diff(delta: &VaspDelta) -> String {
        diff::format_diff(delta)
    }

    pub fn format_batch(outputs: &[(String, VaspOutput, u32, u32)]) -> String {
        vasp::format_batch(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::generate_agent_context;
    use crate::vasp::{compute_position_label, compute_token_savings};
    use farscry_core::{
        Confidence, DeltaEntry, ElementType, ScreenType, StateId, UiElement, VaspDelta, VaspOutput,
    };

    fn create_test_element(text: &str, cx: f32, cy: f32, element_type: ElementType) -> UiElement {
        UiElement {
            text: text.to_string(),
            element_type,
            cx,
            cy,
            w: 100.0,
            h: 30.0,
            enabled: None,
            value: None,
        }
    }

    fn create_test_vasp(elements: Vec<UiElement>) -> VaspOutput {
        VaspOutput::new(
            StateId::from_bits(0x123456789ABCDEF0),
            ScreenType::Config,
            Confidence::High,
            "eng",
            "test context",
            elements,
            vec![],
        )
    }

    #[test]
    fn test_position_label_top_left() {
        let element = create_test_element("Test", 100.0, 100.0, ElementType::Label);
        let label = compute_position_label(&element, 1920, 1080);
        assert_eq!(label, "[top-left]");
    }

    #[test]
    fn test_position_label_middle_center() {
        let element = create_test_element("Test", 960.0, 540.0, ElementType::Label);
        let label = compute_position_label(&element, 1920, 1080);
        assert_eq!(label, "[middle-center]");
    }

    #[test]
    fn test_position_label_bottom_right() {
        let element = create_test_element("Test", 1800.0, 1000.0, ElementType::Label);
        let label = compute_position_label(&element, 1920, 1080);
        assert_eq!(label, "[bottom-right]");
    }

    #[test]
    fn test_agent_context_config() {
        let output = create_test_vasp(vec![
            create_test_element("Max Value:", 200.0, 120.0, ElementType::Label),
            create_test_element("1500", 400.0, 120.0, ElementType::Input),
            create_test_element("Save", 600.0, 200.0, ElementType::Button),
        ]);

        let context = generate_agent_context(&output);
        assert!(context.contains("Config"));
        assert!(context.contains("editable fields"));
    }

    #[test]
    fn test_agent_context_terminal() {
        let mut output = create_test_vasp(vec![create_test_element(
            "Build failed",
            100.0,
            100.0,
            ElementType::Error,
        )]);
        output.screen_type = ScreenType::Terminal;

        let context = generate_agent_context(&output);
        assert!(context.contains("Build failed"));
    }

    #[test]
    fn test_token_savings() {
        let vasp_text = "test output";
        let savings = compute_token_savings(1920, 1080, vasp_text);

        assert!(savings > 2000);
    }

    #[test]
    fn test_vasp_format_output() {
        let output = create_test_vasp(vec![
            create_test_element("Payment Settings", 960.0, 100.0, ElementType::Heading),
            create_test_element("Max Value:", 200.0, 300.0, ElementType::Label),
            create_test_element("1500", 400.0, 300.0, ElementType::Input),
            create_test_element("Save Changes", 600.0, 400.0, ElementType::Button),
        ]);

        let formatted = VaspFormatter::format_vasp(&output, "screenshot.png", 1920, 1080);

        assert!(formatted.contains("=== farscry visual context ==="));
        assert!(formatted.contains("source: screenshot.png"));
        assert!(formatted.contains("screen_type: config"));
        assert!(formatted.contains("state_id: phash:"));
        assert!(formatted.contains("confidence: high"));
        assert!(formatted.contains("lang: eng"));
        assert!(formatted.contains("agent_context:"));
        assert!(formatted.contains("---"));
        assert!(formatted.contains("Token savings:"));
    }

    #[test]
    fn test_json_format_output() {
        let output = create_test_vasp(vec![create_test_element(
            "Test",
            100.0,
            100.0,
            ElementType::Label,
        )]);

        let json = VaspFormatter::format_json(&output, true);

        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_object());
        assert_eq!(parsed["vasp_version"], "1.0");
    }

    #[test]
    fn test_json_roundtrip() {
        let output = create_test_vasp(vec![create_test_element(
            "Test",
            100.0,
            100.0,
            ElementType::Label,
        )]);

        let json = VaspFormatter::format_json(&output, false);
        let parsed: VaspOutput = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.state_id, output.state_id);
        assert_eq!(parsed.ui_tree.len(), output.ui_tree.len());
    }

    #[test]
    fn test_batch_format() {
        let output1 = create_test_vasp(vec![create_test_element(
            "Screen 1",
            100.0,
            100.0,
            ElementType::Label,
        )]);

        let output2 = create_test_vasp(vec![create_test_element(
            "Screen 2",
            100.0,
            100.0,
            ElementType::Label,
        )]);

        let batch = VaspFormatter::format_batch(&[
            ("img1.png".to_string(), output1, 1920, 1080),
            ("img2.png".to_string(), output2, 1920, 1080),
        ]);

        assert!(batch.contains("file: img1.png"));
        assert!(batch.contains("file: img2.png"));
        assert!(batch.contains("---"));
    }

    #[test]
    fn test_diff_format() {
        let delta = VaspDelta {
            vasp_version: "1.0".to_string(),
            diff_from: StateId::from_bits(0x123456789ABCDEF0),
            diff_to: StateId::from_bits(0xFEDCBA9876543210),
            context_similarity: 0.923,
            context_changed: false,
            agent_context: "Test context".to_string(),
            entries: vec![
                DeltaEntry::Appeared(create_test_element(
                    "New Element",
                    100.0,
                    100.0,
                    ElementType::Label,
                )),
                DeltaEntry::Changed {
                    before: create_test_element("Old", 200.0, 200.0, ElementType::Button),
                    after: create_test_element("New", 200.0, 200.0, ElementType::Button),
                },
                DeltaEntry::Removed(create_test_element(
                    "Removed",
                    300.0,
                    300.0,
                    ElementType::Label,
                )),
                DeltaEntry::Unchanged(create_test_element(
                    "Unchanged",
                    400.0,
                    400.0,
                    ElementType::Label,
                )),
            ],
            tokens_saved: Some(3028),
        };

        let formatted = VaspFormatter::format_diff(&delta);

        assert!(formatted.contains("=== farscry diff ==="));
        assert!(formatted.contains("state_id: phash:"));
        assert!(formatted.contains("delta_from: phash:"));
        assert!(formatted.contains("context_similarity: 0.923"));
        assert!(formatted.contains("appeared:"));
        assert!(formatted.contains("changed:"));
        assert!(formatted.contains("removed:"));
        assert!(formatted.contains("unchanged:"));
        assert!(formatted.contains("Token savings:"));
    }

    #[test]
    fn test_format_vasp_affordances_included_by_default() {
        let mut output = create_test_vasp(vec![create_test_element(
            "Save",
            600.0,
            200.0,
            ElementType::Button,
        )]);
        output.screen_type = ScreenType::Ui;

        let formatted = VaspFormatter::format_vasp(&output, "test.png", 1920, 1080);

        assert!(
            formatted.contains("affordances:"));
        assert!(
            formatted.contains("click ->"));
    }

    #[test]
    fn test_format_vasp_affordances_omitted() {
        let mut output = create_test_vasp(vec![create_test_element(
            "Save",
            600.0,
            200.0,
            ElementType::Button,
        )]);
        output.screen_type = ScreenType::Ui;

        let formatted =
            VaspFormatter::format_vasp_with_options(&output, "test.png", 1920, 1080, false);

        assert!(
            !formatted.contains("affordances:"));

        assert!(formatted.contains("Save"));
    }

    #[test]
    fn test_format_text_only() {
        let output = create_test_vasp(vec![
            create_test_element("Error: something failed", 100.0, 100.0, ElementType::Error),
            create_test_element("Retry", 200.0, 200.0, ElementType::Button),
        ]);

        let text = VaspFormatter::format_text_only(&output);
        assert!(text.contains("Error: something failed"));
        assert!(text.contains("Retry"));

        assert!(!text.contains("=== farscry"));
        assert!(!text.contains("state_id:"));
        assert!(!text.contains("affordances:"));
    }

    #[test]
    fn test_format_text_only_filters_empty() {
        let output = create_test_vasp(vec![
            create_test_element("", 100.0, 100.0, ElementType::Label),
            create_test_element("Not empty", 200.0, 200.0, ElementType::Label),
        ]);

        let text = VaspFormatter::format_text_only(&output);

        assert_eq!(text, "Not empty");
    }
}
