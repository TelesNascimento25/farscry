use farscry_core::ScreenType;
use farscry_core::TextRegion;

pub fn detect_screen_type(regions: &[TextRegion]) -> ScreenType {
    detect_screen_type_with_dims(regions, 0.0, 0.0)
}

pub fn detect_screen_type_with_dims(
    regions: &[TextRegion],
    screen_w: f32,
    screen_h: f32,
) -> ScreenType {
    if is_terminal_screen_with_dims(regions, screen_h) {
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

    let _ = screen_w;
    ScreenType::Ui
}

fn is_terminal_screen_with_dims(regions: &[TextRegion], screen_h: f32) -> bool {
    if regions.iter().any(is_strong_terminal_signal) {
        return true;
    }
    let prompt_count = regions
        .iter()
        .filter(|r| is_prompt_region(r, screen_h))
        .count();
    prompt_count >= 2
}

fn is_strong_terminal_signal(region: &TextRegion) -> bool {
    let lower = region.text.to_lowercase();
    if lower.contains("traceback") || lower.contains("file \"") || lower.contains("at line") {
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
}

fn is_prompt_region(region: &TextRegion, screen_h: f32) -> bool {
    let text = region.text.trim();
    let is_prompt = text.starts_with("$ ")
        || text.starts_with("# ")
        || text.starts_with(">>> ")
        || text.starts_with("% ");
    if !is_prompt {
        return false;
    }
    if screen_h > 0.0 && region.cy > screen_h * 0.90 {
        return false;
    }
    true
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
    fn test_terminal_detection_two_prompts() {
        let regions = vec![
            TextRegion {
                text: "$ python3 app.py".to_string(),
                cx: 0.0,
                cy: 0.0,
                w: 200.0,
                h: 20.0,
            },
            TextRegion {
                text: "$ ls -la".to_string(),
                cx: 0.0,
                cy: 25.0,
                w: 100.0,
                h: 20.0,
            },
        ];
        assert_eq!(
            detect_screen_type_with_dims(&regions, 1920.0, 1080.0),
            ScreenType::Terminal
        );
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

    #[test]
    fn test_vscode_dollar_star_in_statusbar_is_not_terminal() {
        let regions = vec![
            TextRegion {
                text: "Visual Studio Code".to_string(),
                cx: 960.0,
                cy: 30.0,
                w: 200.0,
                h: 20.0,
            },
            TextRegion {
                text: "Open Folder...".to_string(),
                cx: 840.0,
                cy: 300.0,
                w: 130.0,
                h: 26.0,
            },
            TextRegion {
                text: "$*".to_string(),
                cx: 100.0,
                cy: 1060.0,
                w: 30.0,
                h: 18.0,
            },
        ];
        let result = detect_screen_type_with_dims(&regions, 1920.0, 1080.0);
        assert_ne!(result, ScreenType::Terminal, "$* in statusbar must not trigger Terminal");
    }

    #[test]
    fn test_single_dollar_prompt_not_terminal() {
        let regions = vec![TextRegion {
            text: "$ python3 app.py".to_string(),
            cx: 0.0,
            cy: 0.0,
            w: 200.0,
            h: 20.0,
        }];
        let result = detect_screen_type_with_dims(&regions, 1920.0, 1080.0);
        assert_ne!(result, ScreenType::Terminal, "single prompt needs >= 2 to be Terminal");
    }

    #[test]
    fn test_slack_hash_channel_not_terminal() {
        let regions = vec![
            TextRegion {
                text: "#general".to_string(),
                cx: 50.0,
                cy: 100.0,
                w: 80.0,
                h: 20.0,
            },
            TextRegion {
                text: "#random".to_string(),
                cx: 50.0,
                cy: 130.0,
                w: 80.0,
                h: 20.0,
            },
        ];
        let result = detect_screen_type_with_dims(&regions, 1920.0, 1080.0);
        assert_ne!(result, ScreenType::Terminal, "#channel must not trigger Terminal");
    }

    #[test]
    fn test_prompt_in_statusbar_not_terminal() {
        let regions = vec![
            TextRegion {
                text: "$ git status".to_string(),
                cx: 100.0,
                cy: 1050.0,
                w: 200.0,
                h: 20.0,
            },
            TextRegion {
                text: "$ git commit".to_string(),
                cx: 100.0,
                cy: 1065.0,
                w: 200.0,
                h: 20.0,
            },
        ];
        let result = detect_screen_type_with_dims(&regions, 1920.0, 1080.0);
        assert_ne!(
            result,
            ScreenType::Terminal,
            "prompts at y>90% of screen (statusbar zone) must not be Terminal"
        );
    }
}
