use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use farscry_core::vasf::VasfFile;
use std::path::PathBuf;

pub fn info(input: PathBuf) -> Result<()> {
    let meta =
        std::fs::metadata(&input).with_context(|| format!("cannot stat {}", input.display()))?;
    let vasf =
        VasfFile::read_from(&input).with_context(|| format!("cannot read {}", input.display()))?;

    let filename = input
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| input.to_string_lossy().into_owned());
    let size_kb = meta.len() / 1024;
    let created = format_ts(vasf.header.created_at);
    let duration = format_duration(vasf.duration_ms());

    println!("file:             {filename} ({size_kb}KB)");
    println!("created:          {created}");
    println!("duration:         {duration}");
    println!();
    println!("frames total:     {}", vasf.total_frames());
    println!("unique states:    {}", vasf.unique_states());
    println!(
        "duplicates:       {} ({:.0}%)",
        vasf.duplicate_count(),
        vasf.dedup_percentage()
    );
    println!();
    println!("tokens (raw):     {}", fmt_number(vasf.tokens_raw()));
    println!("tokens (farscry): {}", fmt_number(vasf.tokens_farscry()));
    println!("reduction:        {:.0}x", vasf.reduction_x());
    println!();
    print_breakdown(&vasf);
    Ok(())
}

fn print_breakdown(vasf: &VasfFile) {
    let mut bd: Vec<(String, u32)> = vasf.screen_type_breakdown().into_iter().collect();
    bd.sort_by_key(|b| std::cmp::Reverse(b.1));
    let total = vasf.unique_states().max(1);
    println!("states breakdown:");
    for (st, count) in &bd {
        let pct = *count as f32 / total as f32 * 100.0;
        let label = capitalize(st);
        let noun = if *count == 1 { "state" } else { "states" };
        println!("  {label:<12}  {count} {noun} ({pct:.0}%)");
    }
}

fn format_ts(ts: i64) -> String {
    DateTime::<Utc>::from_timestamp(ts, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn format_duration(ms: Option<i64>) -> String {
    let ms = match ms {
        Some(v) => v,
        None => return "unknown".to_string(),
    };
    let total_secs = ms / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    if mins > 0 {
        format!("{mins}m {secs}s")
    } else {
        format!("{secs}s")
    }
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

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}
