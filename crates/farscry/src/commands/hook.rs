use anyhow::{Context, Result};
use std::path::PathBuf;

const HOOK_MARKER: &str = "# farscry hook";
const HOOK_EVAL_LINE: &str = "eval \"$(farscry hook init)\"  # farscry hook";

/// On macOS: checks CGPreflightScreenCaptureAccess().
/// If permission is not granted, opens System Settings → Screen Recording,
/// prints onboarding instructions, and exits 0 (not an error — this is the
/// expected first-run flow for any screen-capture tool).
/// On non-macOS platforms this is a no-op.
#[cfg(target_os = "macos")]
fn check_screen_capture_permission() {
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    if unsafe { CGPreflightScreenCaptureAccess() } {
        return;
    }

    let granted_via_dialog = unsafe { CGRequestScreenCaptureAccess() };
    if granted_via_dialog {
        return;
    }

    println!("farscry needs Screen Recording permission to capture your terminal.");
    println!();
    println!("A permission dialog should have appeared — approve it, then run:");
    println!("  farscry setup --hook");
    println!();
    println!("If no dialog appeared:");
    println!("  System Settings → Privacy & Security → Screen Recording");
    println!("  Enable the toggle next to your terminal app");
    println!("  (e.g. iTerm2, Terminal.app, Warp, Ghostty)");
    println!("  Quit and reopen your terminal, then run  farscry setup --hook  again");
    println!();

    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        .status();

    std::process::exit(0);
}

#[cfg(not(target_os = "macos"))]
fn check_screen_capture_permission() {}

const HOOK_SCRIPT: &str = r#"_farscry_session_start() {
  local session_file
  session_file=$(farscry record --daemon --global --pid $$ --silent 2>/dev/null)
  export FARSCRY_SESSION_PID=$$
  export FARSCRY_SESSION_FILE="${session_file}"
}

_farscry_session_stop() {
  [ -n "$FARSCRY_SESSION_PID" ] && \
    farscry daemon unregister "$FARSCRY_SESSION_PID" 2>/dev/null
  unset FARSCRY_SESSION_PID FARSCRY_SESSION_FILE
}

trap '_farscry_session_stop' EXIT
_farscry_session_start"#;

pub fn hook_init() -> Result<()> {
    println!("{HOOK_SCRIPT}");
    Ok(())
}

pub fn setup_hook() -> Result<()> {
    check_screen_capture_permission();

    let rc = detect_rc_file()?;
    let sessions_dir = crate::util::sessions_dir();
    std::fs::create_dir_all(&sessions_dir)?;

    let content = std::fs::read_to_string(&rc).unwrap_or_default();
    if content.contains(HOOK_MARKER) {
        eprintln!("farscry hook already installed in {}", rc.display());
        return Ok(());
    }

    let backup = rc.with_extension("bak");
    std::fs::copy(&rc, &backup).with_context(|| format!("cannot back up {}", rc.display()))?;

    let mut new_content = content;
    if !new_content.ends_with('\n') {
        new_content.push('\n');
    }
    new_content.push('\n');
    new_content.push_str(HOOK_EVAL_LINE);
    new_content.push('\n');
    std::fs::write(&rc, new_content)?;

    println!("farscry hook installed in {}", rc.display());
    println!("Sessions saved to: {}", sessions_dir.display());
    println!("Overhead: <1% CPU  ~18KB/min disk  ~20MB RAM");
    println!("Open a new terminal to start recording.");
    Ok(())
}

pub fn remove_hook() -> Result<()> {
    let rc = detect_rc_file()?;
    let content = std::fs::read_to_string(&rc).unwrap_or_default();
    if !content.contains(HOOK_MARKER) {
        println!("farscry hook not found in {}", rc.display());
        return Ok(());
    }

    let backup = rc.with_extension("bak");
    if backup.exists() {
        std::fs::copy(&backup, &rc)?;
        println!("Restored from backup: {}", backup.display());
    } else {
        let cleaned: String = content
            .lines()
            .filter(|l| !l.contains(HOOK_MARKER))
            .map(|l| format!("{l}\n"))
            .collect();
        std::fs::write(&rc, cleaned)?;
        println!("farscry hook removed from {}", rc.display());
    }
    Ok(())
}

fn detect_rc_file() -> Result<PathBuf> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let shell = std::env::var("SHELL").unwrap_or_default();
    let rc = if shell.contains("zsh") {
        home.join(".zshrc")
    } else if shell.contains("fish") {
        home.join(".config/fish/config.fish")
    } else {
        home.join(".bashrc")
    };
    if !rc.exists() {
        std::fs::write(&rc, "")?;
    }
    Ok(rc)
}
