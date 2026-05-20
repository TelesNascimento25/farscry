use anyhow::{Context, Result};
use farscry_core::{analyze_sessions, FailurePattern, SessionAnalysis};
use std::path::PathBuf;

pub fn analyze(
    paths: Vec<PathBuf>,
    failed: Vec<PathBuf>,
    min_sessions: usize,
    json: bool,
) -> Result<()> {
    if paths.is_empty() {
        anyhow::bail!("no .vasf files provided");
    }
    let all_refs: Vec<&std::path::Path> = paths.iter().map(AsRef::as_ref).collect();
    let failed_refs: Vec<&std::path::Path> = failed.iter().map(AsRef::as_ref).collect();
    let explicit_failed = if failed.is_empty() {
        None
    } else {
        Some(failed_refs.as_slice())
    };
    let result = analyze_sessions(&all_refs, explicit_failed)
        .with_context(|| "failed to analyze sessions")?;
    if result.total_sessions < min_sessions {
        anyhow::bail!(
            "only {} sessions found, minimum required is {}",
            result.total_sessions,
            min_sessions
        );
    }
    if json {
        print_json(&result);
    } else {
        print_human(&result);
    }
    Ok(())
}

fn print_human(r: &SessionAnalysis) {
    let failed = r.failed_sessions;
    let successful = r.total_sessions.saturating_sub(failed);
    println!(
        "Analyzed: {} failed sessions, {} successful sessions",
        failed, successful
    );
    println!();
    println!("ACTION EFFECT RATE (AER)");
    println!("{}", "\u{2500}".repeat(50));
    println!(
        "  AER:    {:.1}%  ({} effects / {} actions)",
        r.aer * 100.0,
        r.ae_count,
        r.total_actions
    );
    println!(
        "  SF rate: {:.1}%  ({} silent failures / {} actions)",
        r.sf_rate * 100.0,
        r.sf_count,
        r.total_actions
    );
    if r.max_consecutive_sf > 0 {
        println!(
            "  Max consecutive SF: {} actions in a row",
            r.max_consecutive_sf
        );
    }
    println!();
    println!("FAILURE PATTERN ANALYSIS");
    println!("{}", "\u{2500}".repeat(50));
    if r.failure_patterns.is_empty() {
        println!("  No failure patterns detected.");
    } else {
        println!("Top states preceding failures:");
        println!();
        for (rank, p) in r.failure_patterns.iter().take(10).enumerate() {
            print_pattern(rank + 1, p);
        }
    }
    println!();
    println!("SILENT FAILURE DETECTION");
    println!("{}", "\u{2500}".repeat(50));
    let sf_pct = percent(r.silent_failure_sessions, failed);
    println!(
        "  {} sessions ({}%) contain silent failures",
        r.silent_failure_sessions, sf_pct
    );
    println!("  Action returned OK. StateId unchanged. Agent continued.");
    println!();
    println!("VISUAL LOOPS");
    println!("{}", "\u{2500}".repeat(50));
    let vl_pct = percent(r.visual_loop_sessions, failed);
    println!(
        "  {} sessions ({}%) contain visual loops",
        r.visual_loop_sessions, vl_pct
    );
    println!("  Sliding window: same StateId 3+ times in 6-step window.");
    println!(
        "  Avg tokens burned in loops: {}/session",
        fmt_number(r.avg_tokens_burned_in_loops as u64)
    );
    println!();
    println!("SESSION SUMMARY");
    println!("{}", "\u{2500}".repeat(50));
    println!("  Total sessions: {}", r.total_sessions);
    let fail_pct = percent(failed, r.total_sessions);
    println!("  Failed: {} ({}%)", failed, fail_pct);
    println!(
        "  Sessions with silent failure: {} ({}% of failed)",
        r.silent_failure_sessions, sf_pct
    );
    println!(
        "  Sessions with visual loop: {} ({}% of failed)",
        r.visual_loop_sessions, vl_pct
    );
    if let Some(top) = r.failure_patterns.first() {
        let top_pct = top.failure_percentage.round() as usize;
        println!(
            "  Most common failure state: {} ({}% of failures)",
            top.state_id, top_pct
        );
    }
}

fn print_pattern(rank: usize, p: &FailurePattern) {
    println!(
        "  {}. StateId {}  \u{2192}  {} failures ({:.0}%)",
        rank, p.state_id, p.failure_count, p.failure_percentage
    );
    println!("     screen_type: {}", p.screen_type);
    println!("     agent_context: \"{}\"", p.agent_context);
    println!(
        "     avg_steps_before_failure: {:.1}",
        p.avg_steps_before_failure
    );
    println!();
}

fn print_json(r: &SessionAnalysis) {
    let failed = r.failed_sessions;
    let successful = r.total_sessions.saturating_sub(failed);
    let sf_pct = percent(r.silent_failure_sessions, failed);
    let vl_pct = percent(r.visual_loop_sessions, failed);
    let top_state = r
        .failure_patterns
        .first()
        .map(|p| p.state_id.to_string())
        .unwrap_or_default();
    let patterns: Vec<serde_json::Value> = r
        .failure_patterns
        .iter()
        .map(|p| {
            serde_json::json!({
                "state_id": p.state_id.to_string(),
                "failure_count": p.failure_count,
                "failure_percentage": p.failure_percentage,
                "screen_type": p.screen_type,
                "agent_context": p.agent_context,
                "avg_steps_before_failure": p.avg_steps_before_failure
            })
        })
        .collect();
    let out = serde_json::json!({
        "total_sessions": r.total_sessions,
        "failed_sessions": failed,
        "successful_sessions": successful,
        "aer": r.aer,
        "aer_pct": (r.aer * 100.0) as u32,
        "sf_rate": r.sf_rate,
        "sf_rate_pct": (r.sf_rate * 100.0) as u32,
        "total_actions": r.total_actions,
        "ae_count": r.ae_count,
        "sf_count": r.sf_count,
        "max_consecutive_sf": r.max_consecutive_sf,
        "silent_failure_sessions": r.silent_failure_sessions,
        "silent_failure_pct": sf_pct,
        "visual_loop_sessions": r.visual_loop_sessions,
        "visual_loop_pct": vl_pct,
        "avg_tokens_burned_in_loops": r.avg_tokens_burned_in_loops,
        "most_common_failure_state": top_state,
        "failure_patterns": patterns
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
}

fn percent(part: usize, total: usize) -> usize {
    if total == 0 {
        return 0;
    }
    (part * 100 + total / 2) / total
}

fn fmt_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}
