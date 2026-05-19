use anyhow::Result;
use std::path::{Path, PathBuf};

use super::terminal::{configure_alacritty, configure_kitty, restore_backup, TerminalResult};

fn write_smart_paste_script(farscry_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(farscry_dir)?;

    #[cfg(not(target_os = "windows"))]
    let script_path = farscry_dir.join("smart-paste.sh");
    #[cfg(target_os = "windows")]
    let script_path = farscry_dir.join("smart-paste.ps1");

    #[cfg(target_os = "macos")]
    let content = r#"#!/bin/bash
HAS_IMAGE=$(osascript -e '
  try
    set img to the clipboard as «class PNGf»
    return "yes"
  end try
  try
    set img to the clipboard as TIFF picture
    return "yes"
  end try
  return "no"
')
if [ "$HAS_IMAGE" = "yes" ]; then
    farscry paste
else
    pbpaste
fi
"#;

    #[cfg(target_os = "linux")]
    let content = r#"#!/bin/bash
if command -v xclip &>/dev/null; then
    HAS_IMAGE=$(xclip -selection clipboard -t TARGETS -o 2>/dev/null | grep -c "image/")
    if [ "$HAS_IMAGE" -gt 0 ]; then
        farscry paste
    else
        xclip -selection clipboard -o
    fi
elif command -v wl-paste &>/dev/null; then
    HAS_IMAGE=$(wl-paste --list-types 2>/dev/null | grep -c "image/")
    if [ "$HAS_IMAGE" -gt 0 ]; then
        farscry paste
    else
        wl-paste
    fi
else
    echo "Install xclip or wl-clipboard for smart paste"
fi
"#;

    #[cfg(target_os = "windows")]
    let content = r#"$formats = [System.Windows.Forms.Clipboard]::GetDataObject().GetFormats()
$hasImage = $formats | Where-Object { $_ -match "Bitmap|PNG|image" }
if ($hasImage) {
    farscry paste
} else {
    Get-Clipboard
}
"#;

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let content = "#!/bin/sh\necho 'Platform not supported'\n";

    std::fs::write(&script_path, content)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))?;
    }

    Ok(script_path)
}

pub fn setup_smart_paste(home: &Path) -> Result<()> {
    let farscry_dir = home.join(".farscry");
    let script = write_smart_paste_script(&farscry_dir)?;
    println!("\nCreated: {}\n", script.display());

    let mut detected: Vec<(&str, bool)> = Vec::new();

    #[cfg(target_os = "macos")]
    {
        let iterm2 = Path::new("/Applications/iTerm.app").exists();
        let warp = Path::new("/Applications/Warp.app").exists();
        let kitty =
            super::wizard::agent_in_path("kitty") || home.join(".config/kitty/kitty.conf").exists();
        let alac = Path::new("/Applications/Alacritty.app").exists()
            || super::wizard::agent_in_path("alacritty");
        detected.push(("iTerm2", iterm2));
        detected.push(("Warp", warp));
        detected.push(("Kitty", kitty));
        detected.push(("Alacritty", alac));
        detected.push(("Terminal.app", true));
    }

    #[cfg(target_os = "linux")]
    {
        detected.push(("Kitty", super::wizard::agent_in_path("kitty")));
        detected.push((
            "Gnome Terminal",
            super::wizard::agent_in_path("gnome-terminal"),
        ));
        detected.push(("Alacritty", super::wizard::agent_in_path("alacritty")));
        detected.push(("Konsole", super::wizard::agent_in_path("konsole")));
        detected.push(("Tilix", super::wizard::agent_in_path("tilix")));
    }

    #[cfg(target_os = "windows")]
    {
        let wt = home.join("AppData/Local/Packages/Microsoft.WindowsTerminal_8wekyb3d8bbwe/LocalState/settings.json");
        detected.push(("Windows Terminal", wt.exists()));
    }

    let found: Vec<&str> = detected
        .iter()
        .filter(|(_, d)| *d)
        .map(|(n, _)| *n)
        .collect();

    if found.is_empty() {
        println!("No terminals detected on your system.");
        return Ok(());
    }

    println!("Found terminals on your system:");
    for (name, det) in &detected {
        if *det {
            let limited = *name == "Terminal.app";
            if limited {
                println!("  \u{2713} {} (limited support)", name);
            } else {
                println!("  \u{2713} {}", name);
            }
        }
    }

    println!("\nConfigure smart Cmd+V in all of them? (y/N)");
    let all = super::wizard::readline_prompt("Configure all? [y/N]: ");

    let to_configure: Vec<&str> = if all.eq_ignore_ascii_case("y") {
        found.clone()
    } else {
        println!("\nWhich terminals to configure?");
        for (i, name) in found.iter().enumerate() {
            println!("  {}. {}", i + 1, name);
        }
        println!("  {}. All of the above", found.len() + 1);
        println!("  {}. Skip", found.len() + 2);

        let choice_str = super::wizard::readline_prompt("Choice: ");
        let choice: usize = choice_str.parse().unwrap_or(found.len() + 2);

        if choice == found.len() + 1 {
            found.clone()
        } else if choice >= 1 && choice <= found.len() {
            vec![found[choice - 1]]
        } else {
            println!("Skipped.");
            return Ok(());
        }
    };

    let mut results: Vec<TerminalResult> = Vec::new();

    for name in &to_configure {
        let result = match *name {
            #[cfg(target_os = "macos")]
            "iTerm2" => super::terminal::configure_iterm2(&script, home),
            #[cfg(target_os = "macos")]
            "Warp" => super::terminal::configure_warp(&script, home),
            "Kitty" => configure_kitty(&script, home),
            "Alacritty" => configure_alacritty(&script, home),
            #[cfg(target_os = "linux")]
            "Gnome Terminal" => super::terminal::configure_bashrc_gnome(&script, home),
            #[cfg(target_os = "windows")]
            "Windows Terminal" => super::terminal::configure_windows_terminal(&script, home),
            "Terminal.app" => TerminalResult {
                name: "Terminal.app",
                configured: false,
                note: "not supported - use fp alias instead",
            },
            other => TerminalResult {
                name: other,
                configured: false,
                note: "unknown terminal",
            },
        };
        results.push(result);
    }

    println!("\nSmart paste configured for:\n");
    for r in &results {
        if r.configured {
            println!("  \u{2713} {:16} -> {}", r.name, r.note);
        } else {
            println!("  \u{2717} {:16} -> {}", r.name, r.note);
        }
    }

    println!("\nTo undo all changes: farscry setup --undo-smart-paste\n");
    println!("Restart your terminal and Cmd+V will");
    println!("automatically detect images.\n");
    println!("Try it: take a screenshot -> press Cmd+V");

    Ok(())
}

pub fn undo_smart_paste_configs(home: &Path) -> Result<()> {
    let mut restored = 0usize;

    let candidates: &[(&str, &[&str])] = &[
        ("iTerm2 plist", &["Library/Preferences/com.googlecode.iterm2.plist"]),
        ("Warp keybindings", &[".warp/keybindings.yaml"]),
        ("kitty.conf", &[".config/kitty/kitty.conf"]),
        ("alacritty", &[".config/alacritty/alacritty.yml", ".config/alacritty/alacritty.toml"]),
        ("~/.bashrc", &[".bashrc"]),
        ("Windows Terminal settings", &["AppData/Local/Packages/Microsoft.WindowsTerminal_8wekyb3d8bbwe/LocalState/settings.json"]),
    ];

    for (label, paths) in candidates {
        for rel in *paths {
            let p = home.join(rel);
            if restore_backup(&p) {
                println!("  \u{2713} Restored {label}");
                restored += 1;
                break;
            }
        }
    }

    if restored == 0 {
        println!("No backups found - nothing to restore.");
    } else {
        println!("\n\u{2713} All terminal configs restored to original.");
    }
    Ok(())
}
