//! Nexus as an MCP server, so an agent running on this computer (Claude Code in a Nexus pane, or any MCP client) can
//! orchestrate the others: list machines and sessions, launch an agent on a machine, type into a session, read what
//! it printed. Opt-in (Settings). Transport: newline-delimited JSON-RPC on a loopback TCP port with a per-run token
//! written to ~/.nexus/mcp.json (mode 600) - only processes running as this user can read it. The `nexus-mcp` binary
//! is the stdio bridge MCP clients spawn: `claude mcp add nexus -- <path>/nexus-mcp`.
use base64::Engine;
use serde_json::{json, Value};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub const PROTOCOL: &str = "2024-11-05";
pub fn info_file() -> PathBuf {
    std::env::var_os("NEXUS_MCP_INFO").map(PathBuf::from).unwrap_or_else(|| crate::platform::home().join(".nexus").join("mcp.json"))
}

/// Terminal output -> plain text (CSI/OSC sequences and carriage returns stripped) for read_output.
pub fn plain(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(c.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] == '\x1b' {
            match c.get(i + 1) {
                Some(']') => { // OSC … BEL | ESC \
                    let mut j = i + 2;
                    while j < c.len() && c[j] != '\x07' && c[j] != '\x1b' { j += 1; }
                    if c.get(j) == Some(&'\x07') { i = j + 1; continue; }
                    if c.get(j) == Some(&'\x1b') && c.get(j + 1) == Some(&'\\') { i = j + 2; continue; }
                }
                Some('[') => { // CSI params, intermediates, final byte
                    let mut j = i + 2;
                    while c.get(j).is_some_and(|x| ('0'..='?').contains(x)) { j += 1; }
                    while c.get(j).is_some_and(|x| (' '..='/').contains(x)) { j += 1; }
                    if c.get(j).is_some_and(|x| ('@'..='~').contains(x)) { i = j + 1; continue; }
                }
                Some('(' | ')') if c.get(i + 2).is_some_and(char::is_ascii_alphanumeric) => { i += 3; continue; }
                Some('=' | '>') => { i += 2; continue; }
                _ => {}
            }
            i += 1; // a lone ESC is a control character: dropped
            continue;
        }
        match c[i] {
            '\r' => { out.push('\n'); if c.get(i + 1) == Some(&'\n') { i += 1; } }
            '\u{0}'..='\u{8}' | '\u{b}'..='\u{1f}' | '\u{7f}' => {}
            ch => out.push(ch),
        }
        i += 1;
    }
    out
}

pub fn tools() -> Value {
    json!([
        { "name": "list_machines", "description": "The machines Nexus knows (this PC, WSL distros, SSH machines) with connection status and the latest GPU/CPU/RAM/disk/network stats.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "list_sessions", "description": "Open terminal sessions: id, machine, preset (claude, agy, openclaw, shell), tmux name, and the last line printed.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "launch", "description": "Start a session on a machine, optionally in a folder. Returns the new session id. presets: claude, antigravity, openclaw, shell, or any preset id from Nexus.", "inputSchema": { "type": "object", "properties": { "machine": { "type": "string", "description": "machine id or name" }, "preset": { "type": "string", "default": "shell" }, "cwd": { "type": "string", "description": "folder on that machine, e.g. ~/proj" } }, "required": ["machine"] } },
        { "name": "send", "description": "Type text into a session (append \"\\n\" to submit). Use for prompts to another agent or shell commands.", "inputSchema": { "type": "object", "properties": { "session": { "type": "string" }, "text": { "type": "string" } }, "required": ["session", "text"] } },
        { "name": "read_output", "description": "The most recent output of a session as plain text (up to 64 KB).", "inputSchema": { "type": "object", "properties": { "session": { "type": "string" }, "max_chars": { "type": "integer", "default": 8000 } }, "required": ["session"] } },
        { "name": "close", "description": "Close a session (remote tmux sessions keep running unless kill is true).", "inputSchema": { "type": "object", "properties": { "session": { "type": "string" }, "kill": { "type": "boolean", "default": false } }, "required": ["session"] } },
    ])
}

pub enum Call {
    ListMachines,
    ListSessions,
    Launch { machine: String, preset: String, cwd: Option<String> },
    Send { session: String, text: String },
    ReadOutput { session: String },
    Close { session: String, kill: bool },
}
pub type CallFuture = Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>;
/// What the app does for each tool call. ReadOutput returns the raw recent output as a JSON string.
pub type Handler = Arc<dyn Fn(Call) -> CallFuture + Send + Sync>;

pub struct McpServer { pub port: u16, task: tokio::task::JoinHandle<()> }

struct RpcError(i64, String);

impl McpServer {
    pub async fn start(version: String, handler: Handler) -> std::io::Result<Self> {
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rand::random::<[u8; 24]>());
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        write_info(&json!({ "port": port, "token": token, "pid": std::process::id() }).to_string())?;
        let task = tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                tokio::spawn(client(sock, token.clone(), version.clone(), handler.clone()));
            }
        });
        Ok(Self { port, task })
    }
    pub fn stop(&self) {
        self.task.abort();
        let ours = std::fs::read_to_string(info_file()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).is_some_and(|v| v["pid"] == std::process::id());
        if ours { let _ = std::fs::remove_file(info_file()); }
    }
}
impl Drop for McpServer { fn drop(&mut self) { self.stop(); } }

fn write_info(text: &str) -> std::io::Result<()> {
    let path = info_file();
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)); }
    }
    let _ = std::fs::remove_file(&path);
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?.write_all(text.as_bytes())
    }
    #[cfg(not(unix))]
    { std::fs::write(&path, text) }
}

async fn client(sock: tokio::net::TcpStream, token: String, version: String, handler: Handler) {
    let (rd, mut wr) = sock.into_split();
    let mut lines = BufReader::new(tokio::io::AsyncReadExt::take(rd, u64::MAX)).lines();
    let mut authed = false;
    while let Ok(Some(line)) = lines.next_line().await {
        if line.len() > 4 * 1024 * 1024 { return; }
        let line = line.trim();
        if line.is_empty() { continue; }
        if !authed { if line == token { authed = true; continue; } return; }
        let reply = match serde_json::from_str::<Value>(line) {
            Err(_) => json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "Parse error" } }),
            Ok(msg) => {
                let Some(id) = msg.get("id").cloned() else { continue }; // notification
                match handle(&msg, &version, &handler).await {
                    Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                    Err(RpcError(code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
                }
            }
        };
        if wr.write_all(format!("{reply}\n").as_bytes()).await.is_err() { return; }
    }
}

async fn handle(msg: &Value, version: &str, handler: &Handler) -> Result<Value, RpcError> {
    let p = &msg["params"];
    match msg["method"].as_str().unwrap_or_default() {
        "initialize" => Ok(json!({ "protocolVersion": PROTOCOL, "capabilities": { "tools": {} }, "serverInfo": { "name": "nexus", "version": version } })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => call(p["name"].as_str().unwrap_or_default(), &p["arguments"], handler).await,
        m => Err(RpcError(-32601, format!("Method not found: {m}"))),
    }
}

async fn call(name: &str, a: &Value, handler: &Handler) -> Result<Value, RpcError> {
    let str_n = |k: &str, n: usize| -> String { match &a[k] { Value::String(s) => s.chars().take(n).collect(), Value::Null => String::new(), v => v.to_string().chars().take(n).collect() } };
    let text = |v: Value| json!({ "content": [{ "type": "text", "text": match v { Value::String(s) => s, v => serde_json::to_string_pretty(&v).unwrap_or_default() } }] });
    let run = |c: Call| async { handler(c).await.map_err(|e| RpcError(-32603, e)) };
    Ok(match name {
        "list_machines" => text(run(Call::ListMachines).await?),
        "list_sessions" => text(run(Call::ListSessions).await?),
        "launch" => {
            let preset = str_n("preset", 24);
            text(run(Call::Launch { machine: str_n("machine", 100), preset: if preset.is_empty() { "shell".into() } else { preset }, cwd: Some(str_n("cwd", 300)).filter(|c| !c.is_empty()) }).await?)
        }
        "send" => { run(Call::Send { session: str_n("session", 32), text: str_n("text", 64 * 1024) }).await?; text(json!("ok")) }
        "read_output" => {
            let raw = run(Call::ReadOutput { session: str_n("session", 32) }).await?;
            let p: Vec<char> = plain(raw.as_str().unwrap_or_default()).chars().collect();
            let max = (a["max_chars"].as_u64().filter(|n| *n > 0).unwrap_or(8000) as usize).clamp(100, 65536);
            text(json!(p[p.len().saturating_sub(max)..].iter().collect::<String>()))
        }
        "close" => { run(Call::Close { session: str_n("session", 32), kill: a["kill"].as_bool().unwrap_or(false) }).await?; text(json!("ok")) }
        n => return Err(RpcError(-32602, format!("Unknown tool: {n}"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, Write};
    use std::sync::Mutex;

    #[test]
    fn plain_strips_terminal_escapes() {
        assert_eq!(plain("\x1b[32mok\x1b[0m\r\n\x1b]0;title\x07done"), "ok\ndone");
        assert_eq!(plain("a\x1b(Bb\x1b=c\rd\x1b]8;;http://x\x1b\\e"), "abc\nde");
    }

    #[test]
    fn token_gated_json_rpc_over_loopback() {
        let info = std::env::temp_dir().join(format!("nexus-mcp-test-{}.json", std::process::id()));
        std::env::set_var("NEXUS_MCP_INFO", &info);
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let sent: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
        let s2 = sent.clone();
        let handler: Handler = Arc::new(move |c| {
            let s2 = s2.clone();
            Box::pin(async move {
                Ok(match c {
                    Call::ListMachines => json!([{ "id": "local", "name": "This PC", "status": "online" }]),
                    Call::ListSessions => json!([{ "id": "s1", "preset": "claude" }]),
                    Call::Launch { machine, preset, cwd } => json!({ "id": "s2", "machine": machine, "preset": preset, "cwd": cwd }),
                    Call::Send { session, text } => { s2.lock().unwrap().push((session, text)); Value::Null }
                    Call::ReadOutput { .. } => json!("\x1b[1mhello\x1b[0m\r\nworld"),
                    Call::Close { .. } => Value::Null,
                })
            })
        });
        let srv = rt.block_on(McpServer::start("9.9.9".into(), handler)).unwrap();
        let meta: Value = serde_json::from_str(&std::fs::read_to_string(&info).unwrap()).unwrap();
        assert_eq!(meta["port"], srv.port);
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; assert_eq!(std::fs::metadata(&info).unwrap().permissions().mode() & 0o777, 0o600); }

        let mut sock = std::net::TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
        let mut rd = std::io::BufReader::new(sock.try_clone().unwrap());
        writeln!(sock, "{}", meta["token"].as_str().unwrap()).unwrap();
        let mut rpc = |id: u32, method: &str, params: Value| -> Value {
            writeln!(sock, "{}", json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).unwrap();
            let mut line = String::new();
            rd.read_line(&mut line).unwrap();
            serde_json::from_str(&line).unwrap()
        };
        assert_eq!(rpc(1, "initialize", json!({}))["result"]["serverInfo"]["version"], "9.9.9");
        assert!(rpc(2, "tools/list", Value::Null)["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "launch"));
        let r = rpc(3, "tools/call", json!({ "name": "launch", "arguments": { "machine": "local", "preset": "claude", "cwd": "~/x" } }));
        assert_eq!(serde_json::from_str::<Value>(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap(), json!({ "id": "s2", "machine": "local", "preset": "claude", "cwd": "~/x" }));
        rpc(4, "tools/call", json!({ "name": "send", "arguments": { "session": "s2", "text": "hi\n" } }));
        assert_eq!(*sent.lock().unwrap(), [("s2".to_string(), "hi\n".to_string())]);
        assert_eq!(rpc(5, "tools/call", json!({ "name": "read_output", "arguments": { "session": "s2" } }))["result"]["content"][0]["text"], "hello\nworld");
        assert_eq!(rpc(6, "tools/call", json!({ "name": "nope", "arguments": {} }))["error"]["code"], -32602);
        assert_eq!(rpc(7, "nope", Value::Null)["error"]["code"], -32601);

        // Wrong token: dropped before any method runs.
        let mut bad = std::net::TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
        writeln!(bad, "bad-token\n{}", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).unwrap();
        let mut any = String::new();
        let _ = std::io::BufReader::new(bad).read_line(&mut any);
        assert!(any.is_empty());
        srv.stop();
        assert!(!info.exists());
    }
}
