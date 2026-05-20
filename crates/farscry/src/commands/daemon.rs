#![cfg(unix)]

use anyhow::{Context, Result};
use chrono::Utc;
use farscry_core::{vasf::VasfWriter, StateId};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
};

#[cfg(target_os = "macos")]
use crate::iosurface_phash as ios;
#[cfg(target_os = "macos")]
use crate::iosurface_phash::DisplayStream;
#[cfg(all(unix, not(target_os = "macos")))]
use libc;

struct WindowEntry {
    #[allow(dead_code)]
    shell_pid: u32,
    last_hash: Option<StateId>,
    writer: VasfWriter,
    session_file: PathBuf,
}

type SharedState = Arc<Mutex<HashMap<u32, WindowEntry>>>;

pub async fn run_daemon() -> Result<()> {
    #[cfg(all(unix, not(target_os = "macos")))]
    let _xvfb = crate::commands::record::ensure_display();

    let pid_path = pid_path();
    let sock_path = sock_path();

    if let Some(p) = pid_path.parent() {
        std::fs::create_dir_all(p)?;
    }

    evict_stale_daemon(&pid_path, &sock_path);

    std::fs::write(&pid_path, std::process::id().to_string())?;

    let listener =
        UnixListener::bind(&sock_path).context("another daemon instance may be running")?;

    let state: SharedState = Arc::new(Mutex::new(HashMap::new()));
    let state_cap = state.clone();

    tokio::spawn(capture_task(state_cap));

    eprintln!("[farscry:daemon] started pid={}", std::process::id());

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let st = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_client(stream, st).await {
                        eprintln!("[farscry:daemon] client error: {e}");
                    }
                });
            }
            Err(e) => {
                eprintln!("[farscry:daemon] accept error: {e}");
                break;
            }
        }
    }

    let _ = std::fs::remove_file(&sock_path);
    let _ = std::fs::remove_file(&pid_path);
    Ok(())
}

pub fn connect_and_register(shell_pid: u32) -> Result<()> {
    ensure_daemon_running()?;

    let sock_path = sock_path();
    let mut stream = std::os::unix::net::UnixStream::connect(&sock_path)
        .context("could not connect to farscry daemon")?;

    use std::io::{BufRead, Write};
    let msg = format!("REGISTER {shell_pid}\n");
    stream.write_all(msg.as_bytes())?;

    let mut resp = String::new();
    std::io::BufReader::new(stream).read_line(&mut resp)?;

    if resp.starts_with("OK ") {
        let tail = resp.trim().trim_start_matches("OK ");
        let (_, file) = tail.split_once(' ').unwrap_or(("0", tail));
        println!("{file}");
        Ok(())
    } else {
        anyhow::bail!("daemon rejected registration: {resp}");
    }
}

pub fn unregister(shell_pid: u32) -> Result<()> {
    let sock_path = sock_path();
    let Ok(mut stream) = std::os::unix::net::UnixStream::connect(&sock_path) else {
        return Ok(());
    };
    use std::io::Write;
    let msg = format!("UNREGISTER {shell_pid}\n");
    stream.write_all(msg.as_bytes()).ok();
    Ok(())
}

async fn handle_client(stream: UnixStream, state: SharedState) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        let parts: Vec<&str> = line.trim().splitn(2, ' ').collect();
        match parts.as_slice() {
            ["REGISTER", pid_str] => {
                let shell_pid: u32 = pid_str.parse().unwrap_or(0);
                match register(shell_pid, &state) {
                    Ok((wid, path)) => {
                        let resp = format!("OK {wid} {}\n", path.display());
                        write_half.write_all(resp.as_bytes()).await?;
                    }
                    Err(e) => {
                        let resp = format!("ERR {e}\n");
                        write_half.write_all(resp.as_bytes()).await?;
                    }
                }
            }
            ["UNREGISTER", pid_str] => {
                let shell_pid: u32 = pid_str.parse().unwrap_or(0);
                drop_window(shell_pid, &state);
                write_half.write_all(b"OK\n").await?;
            }
            ["PING"] => {
                write_half.write_all(b"OK\n").await?;
            }
            _ => {
                write_half.write_all(b"ERR unknown command\n").await?;
            }
        }
    }
    Ok(())
}

fn register(shell_pid: u32, state: &SharedState) -> Result<(u32, PathBuf)> {
    #[cfg(target_os = "macos")]
    let window_id = ios::find_terminal_window(shell_pid).unwrap_or(0);
    #[cfg(not(target_os = "macos"))]
    let window_id: u32 = 0;

    let dir = crate::util::sessions_dir();
    std::fs::create_dir_all(&dir)?;
    let ts = Utc::now().format("%Y%m%d-%H%M%S");
    let file = dir.join(format!("{ts}-{shell_pid}.vasf"));
    let writer = VasfWriter::create(&file)?;

    let entry = WindowEntry {
        shell_pid,
        last_hash: None,
        writer,
        session_file: file.clone(),
    };

    state.lock().unwrap().insert(shell_pid, entry);
    eprintln!(
        "[farscry:daemon] registered pid={shell_pid} window_hint={window_id} → {}",
        file.display()
    );
    Ok((window_id, file))
}

fn drop_window(shell_pid: u32, state: &SharedState) {
    if let Some(mut entry) = state.lock().unwrap().remove(&shell_pid) {
        entry.writer.finalize().ok();
        eprintln!(
            "[farscry:daemon] unregistered pid={shell_pid} → {}",
            entry.session_file.display()
        );
    }
}

async fn capture_task(state: SharedState) {
    let threshold: u8 = 10;
    let mut idle_ticks: u32 = 0;

    #[cfg(target_os = "macos")]
    let stream = DisplayStream::start(2);
    #[cfg(not(target_os = "macos"))]
    let _stream: Option<()> = None;

    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;

        let mut guard = state.lock().unwrap();
        if guard.is_empty() {
            idle_ticks += 1;
            if idle_ticks > 5 {
                eprintln!("[farscry:daemon] no windows registered, exiting");
                drop(guard);
                std::process::exit(0);
            }
            continue;
        }
        idle_ticks = 0;

        #[cfg(target_os = "macos")]
        let current_hash = stream.as_ref().and_then(|s| s.latest_phash());
        #[cfg(not(target_os = "macos"))]
        let current_hash: Option<StateId> = None;

        let Some(hash) = current_hash else {
            continue;
        };
        let ts = crate::util::now_ms();

        for entry in guard.values_mut() {
            let is_new = entry
                .last_hash
                .map(|prev| hash.hamming(prev) > threshold)
                .unwrap_or(true);

            if is_new {
                entry.writer.append_state(hash, "").ok();
                entry.last_hash = Some(hash);
            } else {
                entry.writer.append_timeline(ts, hash).ok();
            }
        }
    }
}

fn evict_stale_daemon(pid_path: &PathBuf, sock_path: &PathBuf) {
    if let Ok(s) = std::fs::read_to_string(pid_path) {
        let pid: u32 = s.trim().parse().unwrap_or(0);
        let alive = {
            #[cfg(target_os = "macos")]
            {
                ios::process_alive(pid)
            }
            #[cfg(all(unix, not(target_os = "macos")))]
            {
                pid > 0 && unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
            }
            #[cfg(not(unix))]
            {
                pid > 0
            }
        };
        if !alive {
            let _ = std::fs::remove_file(sock_path);
            let _ = std::fs::remove_file(pid_path);
        }
    }
}

fn sock_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        ios::daemon_sock_file()
    }
    #[cfg(not(target_os = "macos"))]
    {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".farscry")
            .join("daemon.sock")
    }
}

fn pid_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        ios::daemon_pid_file()
    }
    #[cfg(not(target_os = "macos"))]
    {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".farscry")
            .join("daemon.pid")
    }
}

fn ensure_daemon_running() -> Result<()> {
    use std::{
        io::{BufRead, Write},
        thread,
    };

    let sock_path = sock_path();

    if sock_path.exists() {
        if let Ok(mut s) = std::os::unix::net::UnixStream::connect(&sock_path) {
            if s.write_all(b"PING\n").is_ok() {
                let mut resp = String::new();
                if std::io::BufReader::new(s).read_line(&mut resp).is_ok()
                    && resp.starts_with("OK")
                {
                    return Ok(());
                }
            }
        }
        let _ = std::fs::remove_file(&sock_path);
    }

    let exe = std::env::current_exe()?;
    std::process::Command::new(&exe)
        .args(["daemon", "--start"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("failed to start daemon")?;

    for _ in 0..50 {
        thread::sleep(Duration::from_millis(100));
        if sock_path.exists() {
            if let Ok(mut s) = std::os::unix::net::UnixStream::connect(&sock_path) {
                s.write_all(b"PING\n").ok();
                let mut resp = String::new();
                if std::io::BufReader::new(s).read_line(&mut resp).is_ok()
                    && resp.starts_with("OK")
                {
                    return Ok(());
                }
            }
        }
    }

    anyhow::bail!("daemon did not become ready within 5 s")
}
