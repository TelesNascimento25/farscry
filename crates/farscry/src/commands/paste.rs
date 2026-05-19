use anyhow::{Context, Result};
#[cfg(target_os = "macos")]
use image::GenericImageView;

pub fn paste(agent_override: Option<&str>, prompt_override: Option<&str>) -> Result<()> {
    let cfg = crate::config::read_farscry_config();

    let agent = if let Some(a) = agent_override {
        a.to_string()
    } else if let Some(ref a) = cfg.agent {
        a.preferred.clone()
    } else {
        let choice = crate::commands::setup::readline_prompt(
            "Which agent? (claude / devin / codex) [claude]: ",
        );
        let chosen = if choice.is_empty() {
            "claude".to_string()
        } else {
            choice
        };
        let prompt_default = prompt_override.unwrap_or("fix this").to_string();
        crate::config::write_farscry_config(&chosen, &prompt_default)?;
        println!("Saved. Next time just run: farscry paste\n");
        chosen
    };

    let prompt = prompt_override
        .map(|s| s.to_string())
        .or_else(|| cfg.agent.as_ref().map(|a| a.default_prompt.clone()))
        .unwrap_or_else(|| "fix this".to_string());

    let vasp = capture_clipboard_vasp()?;

    dispatch_to_agent(&agent, &vasp, &prompt)
}

fn capture_clipboard_vasp() -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        let (image_data, _) = crate::clipboard::macos::read_clipboard_image_macos()?;
        let temp_path = std::env::temp_dir().join("farscry_paste.png");
        std::fs::write(&temp_path, image_data)?;
        let output = crate::pipeline::process_image(&temp_path, 50_000_000)?;
        let (w, h) = image::open(&temp_path)
            .map(|i| i.dimensions())
            .unwrap_or((1920, 1080));
        Ok(farscry_formatter::VaspFormatter::format_vasp_with_options(
            &output,
            "clipboard",
            w,
            h,
            true,
        ))
    }

    #[cfg(not(target_os = "macos"))]
    {
        anyhow::bail!("farscry paste currently requires macOS");
    }
}

fn dispatch_to_agent(agent: &str, vasp: &str, prompt: &str) -> Result<()> {
    match agent {
        "claude" => {
            let mut child = std::process::Command::new("claude")
                .args(["-p", prompt])
                .stdin(std::process::Stdio::piped())
                .spawn()
                .context("claude not found in PATH")?;
            if let Some(stdin) = child.stdin.take() {
                use std::io::Write;
                let mut w = stdin;
                writeln!(w, "{vasp}")?;
            }
            child.wait()?;
        }
        "devin" => {
            let full_prompt = format!("{vasp}\n\n{prompt}");
            std::process::Command::new("devin")
                .args(["-p", &full_prompt])
                .status()
                .context("devin not found in PATH")?;
        }
        "codex" => {
            let mut child = std::process::Command::new("codex")
                .args(["exec", prompt])
                .stdin(std::process::Stdio::piped())
                .spawn()
                .context("codex not found in PATH")?;
            if let Some(stdin) = child.stdin.take() {
                use std::io::Write;
                let mut w = stdin;
                writeln!(w, "{vasp}")?;
            }
            child.wait()?;
        }
        other => {
            anyhow::bail!("Unknown agent: {other}. Supported: claude, devin, codex");
        }
    }
    Ok(())
}
