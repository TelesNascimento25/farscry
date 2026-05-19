use farscry_core::ScreenType;
use farscry_core::TextRegion;

pub fn detect_screen_type(regions: &[TextRegion]) -> ScreenType {
    if is_terminal_screen(regions) {
        return ScreenType::Terminal;
    }

    if is_config_screen(regions) {
        return ScreenType::Config;
    }

    if is_conversation_screen(regions) {
        return ScreenType::Conversation;
    }

    if is_error_screen(regions) {
        return ScreenType::Error;
    }

    ScreenType::Ui
}

fn is_terminal_screen(regions: &[TextRegion]) -> bool {
    regions.iter().any(|region| {
        let text = &region.text;
        let lower = text.to_lowercase();

        let prefix_indicators = ["$", "#", "%", ">>>", "Traceback", "File \"", "at line"];
        if prefix_indicators
            .iter()
            .any(|ind| lower.contains(&ind.to_lowercase()))
        {
            return true;
        }

        let needle = "error:";
        let mut start = 0;
        while let Some(pos) = lower[start..].find(needle) {
            let match_pos = start + pos;
            let preceded_by_letter =
                match_pos > 0 && lower.as_bytes()[match_pos - 1].is_ascii_alphabetic();
            if !preceded_by_letter {
                return true;
            }
            start = match_pos + needle.len();
        }

        false
    })
}

fn is_config_screen(regions: &[TextRegion]) -> bool {
    let colon_count = regions
        .iter()
        .filter(|region| region.text.ends_with(':'))
        .count();

    colon_count >= 2
}

fn is_conversation_screen(regions: &[TextRegion]) -> bool {
    if regions.is_empty() {
        return false;
    }

    let short_region_count = regions
        .iter()
        .filter(|region| {
            let word_count = region.text.split_whitespace().count();
            (1..=3).contains(&word_count)
        })
        .count();

    let ratio = short_region_count as f32 / regions.len() as f32;
    ratio >= 0.4
}

fn is_error_screen(regions: &[TextRegion]) -> bool {
    regions.iter().any(|region| {
        let text = region.text.to_lowercase();
        text.contains("error") || text.contains("exception")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_detection_dollar() {
        let regions = vec![TextRegion {
            text: "$ python3 app.py".to_string(),
            cx: 0.0,
            cy: 0.0,
            w: 100.0,
            h: 20.0,
        }];
        assert_eq!(detect_screen_type(&regions), ScreenType::Terminal);
    }

    #[test]
    fn test_terminal_detection_traceback() {
        let regions = vec![TextRegion {
            text: "Traceback (most recent call last):".to_string(),
            cx: 0.0,
            cy: 0.0,
            w: 100.0,
            h: 20.0,
        }];
        assert_eq!(detect_screen_type(&regions), ScreenType::Terminal);
    }

    #[test]
    fn test_config_detection() {
        let regions = vec![
            TextRegion {
                text: "Username:".to_string(),
                cx: 0.0,
                cy: 0.0,
                w: 100.0,
                h: 20.0,
            },
            TextRegion {
                text: "Password:".to_string(),
                cx: 0.0,
                cy: 30.0,
                w: 100.0,
                h: 20.0,
            },
        ];
        assert_eq!(detect_screen_type(&regions), ScreenType::Config);
    }

    #[test]
    fn test_conversation_detection() {
        let regions = vec![
            TextRegion {
                text: "Hi".to_string(),
                cx: 0.0,
                cy: 0.0,
                w: 50.0,
                h: 20.0,
            },
            TextRegion {
                text: "Hello".to_string(),
                cx: 0.0,
                cy: 30.0,
                w: 50.0,
                h: 20.0,
            },
            TextRegion {
                text: "How are you?".to_string(),
                cx: 0.0,
                cy: 60.0,
                w: 100.0,
                h: 20.0,
            },
        ];
        assert_eq!(detect_screen_type(&regions), ScreenType::Conversation);
    }

    #[test]
    fn test_error_detection() {
        let regions = vec![TextRegion {
            text: "An exception occurred while processing".to_string(),
            cx: 0.0,
            cy: 0.0,
            w: 100.0,
            h: 20.0,
        }];
        assert_eq!(detect_screen_type(&regions), ScreenType::Error);
    }

    #[test]
    fn test_terminal_priority_over_error() {
        let regions = vec![
            TextRegion {
                text: "$ python3 app.py".to_string(),
                cx: 0.0,
                cy: 0.0,
                w: 100.0,
                h: 20.0,
            },
            TextRegion {
                text: "Error: command not found".to_string(),
                cx: 0.0,
                cy: 30.0,
                w: 100.0,
                h: 20.0,
            },
        ];
        assert_eq!(detect_screen_type(&regions), ScreenType::Terminal);
    }

    #[test]
    fn test_default_ui() {
        let regions = vec![TextRegion {
            text: "This is a longer text that should not match conversation rules".to_string(),
            cx: 0.0,
            cy: 0.0,
            w: 100.0,
            h: 20.0,
        }];
        assert_eq!(detect_screen_type(&regions), ScreenType::Ui);
    }

    #[test]
    fn test_typeerror_is_error_not_terminal() {
        let regions = vec![TextRegion {
            text: "TypeError: cannot read property 'length' of undefined".to_string(),
            cx: 0.0,
            cy: 0.0,
            w: 300.0,
            h: 20.0,
        }];

        let result = detect_screen_type(&regions);
        assert_ne!(result, ScreenType::Terminal);

        assert_eq!(result, ScreenType::Error);
    }

    #[test]
    fn test_standalone_error_is_terminal() {
        let regions = vec![TextRegion {
            text: "Error: no such file or directory".to_string(),
            cx: 0.0,
            cy: 0.0,
            w: 200.0,
            h: 20.0,
        }];
        assert_eq!(detect_screen_type(&regions), ScreenType::Terminal);
    }
}
