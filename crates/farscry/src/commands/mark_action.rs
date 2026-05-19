use anyhow::{Context, Result};

pub fn mark_action() -> Result<()> {
    let sock = crate::commands::serve::default_socket_path_pub();
    if !sock.exists() {
        anyhow::bail!(
            "no active farscry MCP session (socket: {})\nStart: farscry serve --mcp",
            sock.display()
        );
    }
    #[cfg(unix)]
    {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;
        let payload = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","#,
            r#""params":{"name":"farscry_mark_action","arguments":{}}}"#,
            "\n"
        );
        let mut s =
            UnixStream::connect(&sock).context("could not connect to farscry MCP server")?;
        s.write_all(payload.as_bytes())?;
        s.flush()?;
        let mut buf = String::new();
        BufReader::new(&s).read_line(&mut buf).ok();
    }
    eprintln!("[farscry] action marker written");
    Ok(())
}
