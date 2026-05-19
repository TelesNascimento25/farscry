mod classify;
mod engine;
mod similarity;
mod spatial;
mod text_match;

pub use engine::DiffEngineImpl;

#[cfg(test)]
mod tests {
    use super::classify::compute_tokens_saved;
    use super::engine::DiffEngineImpl;
    use super::similarity::compute_context_similarity;
    use super::spatial::position_proximity;
    use super::text_match::text_similarity;
    use farscry_core::{
        Confidence, DeltaEntry, DiffEngine, ElementType, ScreenType, StateId, UiElement, VaspOutput,
    };

    fn create_test_element(
        text: &str,
        cx: f32,
        cy: f32,
        element_type: ElementType,
        value: Option<String>,
    ) -> UiElement {
        UiElement {
            text: text.to_string(),
            element_type,
            cx,
            cy,
            w: 100.0,
            h: 30.0,
            enabled: None,
            value,
        }
    }

    fn create_test_vasp(elements: Vec<UiElement>) -> VaspOutput {
        VaspOutput::new(
            StateId::from_bits(0x123456789ABCDEF0),
            ScreenType::Ui,
            Confidence::High,
            "eng",
            "test context",
            elements,
            vec![],
        )
    }

    #[test]
    fn test_context_similarity_high() {
        let before = create_test_vasp(vec![
            create_test_element("Username", 50.0, 100.0, ElementType::Label, None),
            create_test_element("Password", 50.0, 150.0, ElementType::Label, None),
        ]);

        let after = create_test_vasp(vec![
            create_test_element("Username", 50.0, 100.0, ElementType::Label, None),
            create_test_element("Password", 50.0, 150.0, ElementType::Label, None),
        ]);

        let sim = compute_context_similarity(&before, &after);
        assert!(sim > 0.8);
    }

    #[test]
    fn test_context_similarity_low() {
        let before = create_test_vasp(vec![create_test_element(
            "Username",
            50.0,
            100.0,
            ElementType::Label,
            None,
        )]);

        let after = create_test_vasp(vec![create_test_element(
            "Completely different text",
            50.0,
            100.0,
            ElementType::Label,
            None,
        )]);

        let sim = compute_context_similarity(&before, &after);
        assert!(sim < 0.5);
    }

    #[test]
    fn test_text_similarity() {
        assert!(text_similarity("Username", "Username") > 0.99);
        assert!(text_similarity("Username", "Password") < 0.5);

        assert!(text_similarity("Hello World", "Hello") > 0.3);
        assert!(text_similarity("Hello World", "Hello") < 0.6);
    }

    #[test]
    fn test_position_proximity() {
        let elem1 = create_test_element("Test", 100.0, 100.0, ElementType::Label, None);
        let elem2 = create_test_element("Test", 100.0, 100.0, ElementType::Label, None);
        let offset = (0.0, 0.0);

        let sim = position_proximity(&elem1, &elem2, &offset);
        assert!(sim > 0.9);
    }

    #[test]
    fn test_position_proximity_scrolled() {
        let elem1 = create_test_element("Test", 100.0, 100.0, ElementType::Label, None);
        let elem2 = create_test_element("Test", 100.0, 340.0, ElementType::Label, None);
        let offset = (0.0, 240.0);

        let sim = position_proximity(&elem1, &elem2, &offset);
        assert!(sim > 0.9);
    }

    #[test]
    fn test_scroll_detection() {
        let before = create_test_vasp(vec![
            create_test_element("Dashboard", 50.0, 100.0, ElementType::Heading, None),
            create_test_element("Users", 50.0, 150.0, ElementType::Label, None),
            create_test_element("Settings", 200.0, 150.0, ElementType::Label, None),
            create_test_element("Logout", 350.0, 150.0, ElementType::Button, None),
        ]);

        let after = create_test_vasp(vec![
            create_test_element("Users", 50.0, 390.0, ElementType::Label, None),
            create_test_element("Settings", 200.0, 390.0, ElementType::Label, None),
            create_test_element("Logout", 350.0, 390.0, ElementType::Button, None),
            create_test_element("Reports", 50.0, 440.0, ElementType::Label, None),
        ]);

        let diff_engine = DiffEngineImpl;
        let delta = diff_engine.diff(&before, &after, None, None);

        assert!(!delta.context_changed);

        let unchanged_count = delta
            .entries
            .iter()
            .filter(|e| matches!(e, DeltaEntry::Unchanged(_)))
            .count();
        let removed_count = delta
            .entries
            .iter()
            .filter(|e| matches!(e, DeltaEntry::Removed(_)))
            .count();
        let appeared_count = delta
            .entries
            .iter()
            .filter(|e| matches!(e, DeltaEntry::Appeared(_)))
            .count();
        let changed_count = delta
            .entries
            .iter()
            .filter(|e| matches!(e, DeltaEntry::Changed { .. }))
            .count();

        assert!(unchanged_count > 0);

        assert!(removed_count > 0);

        assert!(appeared_count > 0);

        assert_eq!(
            changed_count, 0);
    }

    #[test]
    fn test_field_filled() {
        let before = create_test_vasp(vec![
            create_test_element("Email", 50.0, 100.0, ElementType::Label, None),
            create_test_element("Enter email", 150.0, 100.0, ElementType::Input, None),
            create_test_element("Password", 50.0, 150.0, ElementType::Label, None),
            create_test_element("•••••••", 150.0, 150.0, ElementType::Input, None),
            create_test_element("Submit", 50.0, 200.0, ElementType::Button, None),
        ]);

        let after = create_test_vasp(vec![
            create_test_element("Email", 50.0, 100.0, ElementType::Label, None),
            create_test_element("Email entered", 150.0, 100.0, ElementType::Input, None),
            create_test_element("Password", 50.0, 150.0, ElementType::Label, None),
            create_test_element("•••••••", 150.0, 150.0, ElementType::Input, None),
            create_test_element("Submit", 50.0, 200.0, ElementType::Button, None),
        ]);

        let diff_engine = DiffEngineImpl;
        let delta = diff_engine.diff(&before, &after, None, None);

        assert!(!delta.context_changed);

        let changed_count = delta
            .entries
            .iter()
            .filter(|e| matches!(e, DeltaEntry::Changed { .. }))
            .count();
        let unchanged_count = delta
            .entries
            .iter()
            .filter(|e| matches!(e, DeltaEntry::Unchanged(_)))
            .count();

        assert!(
            changed_count >= 1);

        assert!(
            unchanged_count >= 3);
    }

    #[test]
    fn test_error_appeared() {
        let before = create_test_vasp(vec![
            create_test_element("Card Number", 50.0, 100.0, ElementType::Label, None),
            create_test_element("", 150.0, 100.0, ElementType::Input, None),
            create_test_element("Expiry", 50.0, 150.0, ElementType::Label, None),
            create_test_element("", 150.0, 150.0, ElementType::Input, None),
            create_test_element("Pay", 50.0, 200.0, ElementType::Button, None),
        ]);

        let after = create_test_vasp(vec![
            create_test_element("Card Number", 50.0, 100.0, ElementType::Label, None),
            create_test_element("", 150.0, 100.0, ElementType::Input, None),
            create_test_element("Expiry", 50.0, 150.0, ElementType::Label, None),
            create_test_element("", 150.0, 150.0, ElementType::Input, None),
            create_test_element("Pay", 50.0, 200.0, ElementType::Button, None),
            create_test_element(
                "Error: Invalid card number",
                50.0,
                250.0,
                ElementType::Error,
                None,
            ),
        ]);

        let diff_engine = DiffEngineImpl;
        let delta = diff_engine.diff(&before, &after, None, None);

        assert!(!delta.context_changed);

        let appeared_count = delta
            .entries
            .iter()
            .filter(|e| matches!(e, DeltaEntry::Appeared(_)))
            .count();
        let unchanged_count = delta
            .entries
            .iter()
            .filter(|e| matches!(e, DeltaEntry::Unchanged(_)))
            .count();
        let changed_count = delta
            .entries
            .iter()
            .filter(|e| matches!(e, DeltaEntry::Changed { .. }))
            .count();

        assert_eq!(appeared_count, 1);

        assert_eq!(unchanged_count, 5);

        assert_eq!(changed_count, 0);
    }

    #[test]
    fn test_context_gate() {
        let before = create_test_vasp(vec![
            create_test_element("Card Number", 50.0, 100.0, ElementType::Label, None),
            create_test_element("Pay", 50.0, 200.0, ElementType::Button, None),
        ]);

        let after = create_test_vasp(vec![
            create_test_element(
                "Welcome to Dashboard",
                50.0,
                100.0,
                ElementType::Heading,
                None,
            ),
            create_test_element(
                "Your balance is $1000",
                50.0,
                150.0,
                ElementType::Label,
                None,
            ),
            create_test_element("View Transactions", 50.0, 200.0, ElementType::Button, None),
        ]);

        let diff_engine = DiffEngineImpl;
        let delta = diff_engine.diff(&before, &after, None, None);

        assert!(delta.context_changed);
        assert!(delta.context_similarity < 0.20);

        assert_eq!(delta.entries.len(), 0);
    }

    #[test]
    fn test_token_savings() {
        assert!(compute_tokens_saved(None, None).is_none());

        let saved = compute_tokens_saved(Some((1920, 1080)), Some((1920, 1080)));
        assert!(saved.is_some());
        let saved = saved.unwrap();
        assert!(
            saved > 5000);

        let small = compute_tokens_saved(Some((640, 480)), None);
        assert!(small.is_some());

        let tiny = compute_tokens_saved(Some((100, 100)), None);
        assert_eq!(tiny, Some(0));
    }

    #[test]
    fn test_token_savings_in_delta() {
        let diff_engine = DiffEngineImpl;

        let before = create_test_vasp(vec![create_test_element(
            "Save",
            50.0,
            100.0,
            ElementType::Button,
            None,
        )]);
        let after = create_test_vasp(vec![create_test_element(
            "Save",
            50.0,
            100.0,
            ElementType::Button,
            None,
        )]);

        let delta_no_dims = diff_engine.diff(&before, &after, None, None);
        assert!(delta_no_dims.tokens_saved.is_none());

        let delta_with_dims =
            diff_engine.diff(&before, &after, Some((1920, 1080)), Some((1920, 1080)));
        assert!(delta_with_dims.tokens_saved.is_some());
        assert!(delta_with_dims.tokens_saved.unwrap() > 5000);
    }
}
