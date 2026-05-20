use anyhow::{Context, Result};

pub fn mark_action() -> Result<()> {
    let mcp_sock = crate::commands::serve::default_socket_path_pub();
    let daemon_sock = crate::util::sessions_dir()
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("daemon.sock");

    #[cfg(unix)]
    {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;

        if mcp_sock.exists() {
            let payload = concat!(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","#,
                r#""params":{"name":"farscry_mark_action","arguments":{}}}"#,
                "\n"
            );
            let mut s = UnixStream::connect(&mcp_sock)
                .context("could not connect to farscry MCP server")?;
            s.write_all(payload.as_bytes())?;
            s.flush()?;
            let mut buf = String::new();
            BufReader::new(&s).read_line(&mut buf).ok();
            eprintln!("[farscry] action marker written (MCP)");
            return Ok(());
        }

        if daemon_sock.exists() {
            let msg = format!("MARK_ACTION {}\n", std::process::id());
            if let Ok(mut s) = UnixStream::connect(&daemon_sock) {
                let _ = s.write_all(msg.as_bytes());
                s.flush().ok();
                eprintln!("[farscry] action marker written (daemon)");
                return Ok(());
            }
        }
    }

    eprintln!(
        "[farscry] warning: no active session — start farscry serve --mcp or farscry record --daemon"
    );
    Ok(())
}
