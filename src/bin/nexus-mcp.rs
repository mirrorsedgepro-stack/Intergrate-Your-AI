//! Nexus MCP bridge: an MCP stdio server that forwards to the running Nexus app on this computer.
//! Register it with an MCP client, e.g. Claude Code:  claude mcp add nexus -- /path/to/nexus-mcp
//! Nexus must be running with "Let MCP clients on this PC control Nexus" enabled in Settings; it writes the port and a
//! per-run token to ~/.nexus/mcp.json, which this bridge reads.
use std::io::{BufRead, Read, Write};
use std::net::{Shutdown, TcpStream};

fn main() {
    let info = std::env::var_os("NEXUS_MCP_INFO").map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|h| std::path::PathBuf::from(h).join(".nexus").join("mcp.json")));
    let parsed = info.and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| {
        // {"port":1234,"token":"…","pid":1}: tiny and written by Nexus, so pick the two fields out by hand (no JSON dependency in the bridge).
        let field = |k: &str| { let i = t.find(&format!("\"{k}\":"))? + k.len() + 3; let rest = t[i..].trim_start_matches('"'); Some(rest[..rest.find(['"', ',', '}'])?].to_string()) };
        Some((field("port")?.parse::<u16>().ok()?, field("token")?))
    });
    let Some((port, token)) = parsed else {
        eprintln!("nexus-mcp: Nexus is not running, or MCP is not enabled in its Settings (no ~/.nexus/mcp.json)");
        std::process::exit(1);
    };
    let mut sock = match TcpStream::connect(("127.0.0.1", port)) {
        Ok(s) => s,
        Err(e) => { eprintln!("nexus-mcp: cannot reach Nexus ({e})"); std::process::exit(1); }
    };
    let _ = sock.set_nodelay(true);
    if writeln!(sock, "{token}").is_err() { std::process::exit(1); }

    // socket -> stdout
    let mut from = sock.try_clone().expect("socket");
    let reader = std::thread::spawn(move || {
        let (mut buf, mut out) = ([0u8; 16 * 1024], std::io::stdout());
        while let Ok(n) = from.read(&mut buf) {
            if n == 0 || out.write_all(&buf[..n]).is_err() || out.flush().is_err() { break; }
        }
        std::process::exit(0); // Nexus closed the connection (or quit)
    });
    // stdio -> socket: MCP stdio is newline-delimited JSON, same as the socket; forward lines as they are.
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if !line.trim().is_empty() && writeln!(sock, "{line}").is_err() { break; }
    }
    let _ = sock.shutdown(Shutdown::Write);
    let _ = reader.join();
}
