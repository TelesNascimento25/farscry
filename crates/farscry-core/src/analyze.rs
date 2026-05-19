use crate::types::{ScreenType, StateId};
use crate::vasf::{is_action_marker, VasfFile};
use std::collections::HashMap;
use std::path::Path;

const TOKENS_PER_VASF_FRAME: u64 = 900;

pub struct SessionAnalysis {
    pub total_sessions: usize,
    pub failed_sessions: usize,
    pub silent_failure_sessions: usize,
    pub visual_loop_sessions: usize,
    pub failure_patterns: Vec<FailurePattern>,
    pub avg_tokens_burned_in_loops: f64,
}

pub struct FailurePattern {
    pub state_id: StateId,
    pub failure_count: usize,
    pub failure_percentage: f32,
    pub screen_type: String,
    pub agent_context: String,
    pub avg_steps_before_failure: f32,
}

struct ParsedSession {
    state_ids: Vec<StateId>,
    terminal_screen_type: ScreenType,
    terminal_screen_type_str: String,
    terminal_agent_context: String,
    has_silent_failure: bool,
    loop_tokens_burned: u64,
    has_visual_loop: bool,
}

pub fn analyze_sessions(
    all_paths: &[&Path],
    explicit_failed: Option<&[&Path]>,
) -> Result<SessionAnalysis, std::io::Error> {
    let sessions = load_sessions(all_paths)?;
    let failed_set: Option<std::collections::HashSet<&Path>> =
        explicit_failed.map(|paths| paths.iter().copied().collect());
    classify_and_analyze(&sessions, all_paths, failed_set.as_ref())
}

fn load_sessions(paths: &[&Path]) -> Result<Vec<ParsedSession>, std::io::Error> {
    paths.iter().map(|p| load_one(p)).collect()
}

fn load_one(path: &Path) -> Result<ParsedSession, std::io::Error> {
    let vasf = VasfFile::read_from(path)?;

    let state_ids: Vec<StateId> = vasf
        .frames
        .iter()
        .filter(|f| !is_action_marker(f))
        .map(|f| f.state_id)
        .collect();

    let (terminal_screen_type_str, terminal_agent_context) = vasf
        .frames
        .iter()
        .filter(|f| !is_action_marker(f))
        .last()
        .map(|f| {
            let text = std::str::from_utf8(&f.vasp_data).unwrap_or("");
            (
                vasp_field(text, "screen_type: ").to_string(),
                vasp_field(text, "agent_context: ").to_string(),
            )
        })
        .unwrap_or_default();

    let terminal_screen_type = parse_screen_type(&terminal_screen_type_str);
    let has_silent_failure = detect_silent_failure(&vasf.frames);
    let (has_visual_loop, loop_tokens_burned) = detect_visual_loop(&state_ids);

    Ok(ParsedSession {
        state_ids,
        terminal_screen_type,
        terminal_screen_type_str,
        terminal_agent_context,
        has_silent_failure,
        has_visual_loop,
        loop_tokens_burned,
    })
}

fn detect_silent_failure(frames: &[crate::vasf::VasfFrame]) -> bool {
    let action_marker_indices: Vec<usize> = frames
        .iter()
        .enumerate()
        .filter(|(_, f)| is_action_marker(f))
        .map(|(i, _)| i)
        .collect();

    if action_marker_indices.is_empty() {
        return false;
    }

    for &marker_idx in &action_marker_indices {
        let state_before = frames[..marker_idx]
            .iter()
            .rev()
            .find(|f| !is_action_marker(f))
            .map(|f| f.state_id);

        let state_after = frames[marker_idx + 1..]
            .iter()
            .find(|f| !is_action_marker(f))
            .map(|f| f.state_id);

        if let (Some(before), Some(after)) = (state_before, state_after) {
            if before == after {
                return true;
            }
        }
    }
    false
}

fn classify_and_analyze(
    sessions: &[ParsedSession],
    all_paths: &[&Path],
    failed_set: Option<&std::collections::HashSet<&Path>>,
) -> Result<SessionAnalysis, std::io::Error> {
    let mut failed_indices: Vec<usize> = Vec::new();
    for (i, session) in sessions.iter().enumerate() {
        let is_failed = match failed_set {
            Some(set) => set.contains(all_paths[i]),
            None => session.terminal_screen_type == ScreenType::Error,
        };
        if is_failed {
            failed_indices.push(i);
        }
    }
    let total_sessions = sessions.len();
    let failed_sessions = failed_indices.len();
    let failed_subset: Vec<&ParsedSession> = failed_indices.iter().map(|&i| &sessions[i]).collect();
    let silent_failure_sessions = failed_subset
        .iter()
        .filter(|s| s.has_silent_failure)
        .count();
    let visual_loop_sessions = failed_subset.iter().filter(|s| s.has_visual_loop).count();
    let total_loop_tokens: u64 = failed_subset
        .iter()
        .filter(|s| s.has_visual_loop)
        .map(|s| s.loop_tokens_burned)
        .sum();
    let avg_tokens_burned_in_loops = if visual_loop_sessions > 0 {
        total_loop_tokens as f64 / visual_loop_sessions as f64
    } else {
        0.0
    };
    let failure_patterns = build_failure_patterns(&failed_subset, failed_sessions);
    Ok(SessionAnalysis {
        total_sessions,
        failed_sessions,
        silent_failure_sessions,
        visual_loop_sessions,
        failure_patterns,
        avg_tokens_burned_in_loops,
    })
}

fn build_failure_patterns(failed: &[&ParsedSession], total_failed: usize) -> Vec<FailurePattern> {
    if total_failed == 0 {
        return Vec::new();
    }
    let mut terminal_map: HashMap<StateId, Vec<usize>> = HashMap::new();
    for (i, session) in failed.iter().enumerate() {
        if let Some(&last) = session.state_ids.last() {
            terminal_map.entry(last).or_default().push(i);
        }
    }
    let mut patterns: Vec<FailurePattern> = terminal_map
        .into_iter()
        .map(|(state_id, session_indices)| {
            let failure_count = session_indices.len();
            let failure_percentage = failure_count as f32 / total_failed as f32 * 100.0;
            let sample = &failed[session_indices[0]];
            let avg_steps = session_indices
                .iter()
                .map(|&i| failed[i].state_ids.len() as f32)
                .sum::<f32>()
                / session_indices.len() as f32;
            FailurePattern {
                state_id,
                failure_count,
                failure_percentage,
                screen_type: sample.terminal_screen_type_str.clone(),
                agent_context: sample.terminal_agent_context.clone(),
                avg_steps_before_failure: avg_steps,
            }
        })
        .collect();
    patterns.sort_by(|a, b| b.failure_count.cmp(&a.failure_count));
    patterns
}

fn detect_visual_loop(state_ids: &[StateId]) -> (bool, u64) {
    let mut counts: HashMap<StateId, usize> = HashMap::new();
    for &id in state_ids {
        *counts.entry(id).or_insert(0) += 1;
    }
    let extra_visits: usize = counts.values().filter(|&&c| c >= 3).map(|&c| c - 1).sum();
    let has_loop = counts.values().any(|&c| c >= 3);
    (has_loop, extra_visits as u64 * TOKENS_PER_VASF_FRAME)
}

fn vasp_field<'a>(text: &'a str, prefix: &str) -> &'a str {
    text.lines()
        .find_map(|line| line.strip_prefix(prefix))
        .map(|v| v.trim().trim_matches('"'))
        .unwrap_or("unknown")
}

fn parse_screen_type(s: &str) -> ScreenType {
    match s.to_ascii_lowercase().as_str() {
        "error" => ScreenType::Error,
        "config" => ScreenType::Config,
        "terminal" => ScreenType::Terminal,
        "conversation" => ScreenType::Conversation,
        "ui" => ScreenType::Ui,
        _ => ScreenType::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vasf::{VasfFile, VasfFrame, VasfWriter};

    fn make_frame(state_bits: u64, screen_type: &str, agent_ctx: &str) -> VasfFrame {
        VasfFrame {
            state_id: StateId::from_bits(state_bits),
            timestamp: 0,
            vasp_data: format!("screen_type: {screen_type}\nagent_context: \"{agent_ctx}\"\n")
                .into_bytes(),
            delta_data: None,
        }
    }

    fn tmp_vasf(label: &str, frames: Vec<VasfFrame>, total_input: u32) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("_analyze_{label}.vasf"));
        VasfFile::new(frames, total_input).write_to(&path).unwrap();
        path
    }

    fn tmp_vasf_writer(label: &str) -> (VasfWriter, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("_analyze_{label}.vasf"));
        let w = VasfWriter::create(&path).unwrap();
        (w, path)
    }

    #[test]
    fn test_failed_by_error_screen_type() {
        let path = tmp_vasf(
            "failed_by_error",
            vec![
                make_frame(1, "ui", "working"),
                make_frame(2, "error", "failed here"),
            ],
            2,
        );
        let paths: Vec<&Path> = vec![path.as_path()];
        let result = analyze_sessions(&paths, None).unwrap();
        assert_eq!(result.failed_sessions, 1);
        assert_eq!(result.total_sessions, 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_successful_session_not_counted_as_failed() {
        let path = tmp_vasf(
            "successful_not_failed",
            vec![
                make_frame(1, "ui", "navigating"),
                make_frame(2, "config", "done"),
            ],
            2,
        );
        let paths: Vec<&Path> = vec![path.as_path()];
        let result = analyze_sessions(&paths, None).unwrap();
        assert_eq!(result.failed_sessions, 0);
        assert_eq!(result.total_sessions, 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_visual_loop_detection() {
        let (has_loop, tokens) = detect_visual_loop(&[
            StateId::from_bits(1),
            StateId::from_bits(2),
            StateId::from_bits(1),
            StateId::from_bits(2),
            StateId::from_bits(1),
        ]);
        assert!(has_loop);
        assert!(tokens > 0);
    }

    #[test]
    fn test_no_visual_loop_when_state_appears_twice() {
        let (has_loop, _) = detect_visual_loop(&[
            StateId::from_bits(1),
            StateId::from_bits(2),
            StateId::from_bits(1),
        ]);
        assert!(!has_loop);
    }

    #[test]
    fn test_no_silent_failure_without_action_markers() {
        let path = tmp_vasf("no_sf_no_markers", vec![make_frame(1, "error", "stuck")], 5);
        let paths: Vec<&Path> = vec![path.as_path()];
        let result = analyze_sessions(&paths, None).unwrap();
        assert_eq!(result.silent_failure_sessions, 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_silent_failure_detected_via_action_marker() {
        let (mut w, path) = tmp_vasf_writer("sf_with_marker");
        let same_id = StateId::from_bits(0xDEAD);
        let vasp = format!("screen_type: error\nagent_context: \"stuck\"\n");
        w.append_state(same_id, &vasp).unwrap();
        w.append_action_marker().unwrap();
        w.append_state(same_id, &vasp).unwrap();
        w.finalize().unwrap();

        let paths: Vec<&Path> = vec![path.as_path()];
        let result = analyze_sessions(&paths, Some(&[path.as_path()])).unwrap();
        assert_eq!(result.silent_failure_sessions, 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_no_silent_failure_when_action_changes_state() {
        let (mut w, path) = tmp_vasf_writer("sf_marker_changed");
        let before = StateId::from_bits(0xAAAA);
        let after = StateId::from_bits(0xBBBB);
        let vasp_before = "screen_type: ui\nagent_context: \"before\"\n";
        let vasp_after = "screen_type: error\nagent_context: \"after\"\n";
        w.append_state(before, vasp_before).unwrap();
        w.append_action_marker().unwrap();
        w.append_state(after, vasp_after).unwrap();
        w.finalize().unwrap();

        let paths: Vec<&Path> = vec![path.as_path()];
        let result = analyze_sessions(&paths, Some(&[path.as_path()])).unwrap();
        assert_eq!(result.silent_failure_sessions, 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_explicit_failed_flag() {
        let ok_path = tmp_vasf("explicit_ok", vec![make_frame(1, "config", "success")], 1);
        let fail_path = tmp_vasf(
            "explicit_fail",
            vec![make_frame(2, "config", "also config but explicitly failed")],
            5,
        );
        let all: Vec<&Path> = vec![ok_path.as_path(), fail_path.as_path()];
        let explicit: Vec<&Path> = vec![fail_path.as_path()];
        let result = analyze_sessions(&all, Some(&explicit)).unwrap();
        assert_eq!(result.total_sessions, 2);
        assert_eq!(result.failed_sessions, 1);
        let _ = std::fs::remove_file(&ok_path);
        let _ = std::fs::remove_file(&fail_path);
    }

    #[test]
    fn test_failure_pattern_built_from_terminal_state() {
        let path = tmp_vasf(
            "failure_pattern_terminal",
            vec![
                make_frame(0xAA, "ui", "step 1"),
                make_frame(0xBB, "error", "broke here"),
            ],
            2,
        );
        let paths: Vec<&Path> = vec![path.as_path()];
        let result = analyze_sessions(&paths, None).unwrap();
        assert_eq!(result.failure_patterns.len(), 1);
        assert_eq!(
            result.failure_patterns[0].state_id,
            StateId::from_bits(0xBB)
        );
        assert_eq!(result.failure_patterns[0].failure_count, 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_avg_tokens_burned_zero_when_no_loops() {
        let path = tmp_vasf(
            "tokens_burned_no_loops",
            vec![make_frame(1, "ui", "a"), make_frame(2, "error", "b")],
            2,
        );
        let paths: Vec<&Path> = vec![path.as_path()];
        let result = analyze_sessions(&paths, None).unwrap();
        assert_eq!(result.avg_tokens_burned_in_loops, 0.0);
        let _ = std::fs::remove_file(&path);
    }
}
