//! Team chat + presence, stored on one "hub" SSH machine under ~/.nexus/collab and reached over each Nexus's existing SSH
//! connection: no extra server, no extra ports, and only people who can reach those machines can read it.
//!   chat.jsonl          one JSON message per line, appended atomically (lines are kept well under PIPE_BUF)
//!   presence/<id>.json  each Nexus's heartbeat: who, status line, which sessions it has open
//! This file holds the data model and the remote commands; the hub connection itself lives in `backend`.
use crate::util::{clip, now_ms};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DIR: &str = "~/.nexus/collab";
pub const MAX_TEXT: usize = 1500;
const PRESENCE_TTL_MIN: u32 = 2;

/// Strip control characters (keeping tab and newline) and cap the length.
fn clean(s: &str, n: usize) -> String {
    s.chars().filter(|&c| !matches!(c, '\u{0}'..='\u{8}' | '\u{b}'..='\u{1f}' | '\u{7f}')).take(n).collect()
}
fn vstr(v: &Value) -> &str { v.as_str().unwrap_or_default() }

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct From { pub name: String, pub login: String, pub client: String }

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message { pub id: String, pub ts: i64, pub kind: String, pub from: From, pub text: String }

pub fn make_message(from: &From, text: &str, kind: &str) -> Message {
    let name = clean(&from.name, 40);
    Message {
        id: (0..6).map(|_| format!("{:02x}", rand::random::<u8>())).collect(),
        ts: now_ms(),
        kind: if ["msg", "status", "activity"].contains(&kind) { kind } else { "msg" }.to_string(),
        from: From { name: if name.is_empty() { "someone".into() } else { name }, login: clean(&from.login, 80), client: clean(&from.client, 24) },
        text: clean(text, MAX_TEXT).trim().to_string(),
    }
}

/// Parse one chat.jsonl line; anything malformed (or hand-edited badly) is dropped rather than trusted.
pub fn parse_line(line: &str) -> Option<Message> {
    let m: Value = serde_json::from_str(line).ok()?;
    let (id, ts, text) = (m["id"].as_str()?, m["ts"].as_f64()?, m["text"].as_str()?);
    let f = m.get("from").filter(|f| f.is_object())?;
    let from = From { name: vstr(&f["name"]).into(), login: vstr(&f["login"]).into(), client: vstr(&f["client"]).into() };
    Some(Message { id: clean(id, 24), ts: ts as i64, ..make_message(&from, text, m["kind"].as_str().unwrap_or("msg")) })
}

/// A GPU / machine claim: "I'm using <host> until <time>". Carried in presence, so it expires with the person's
/// heartbeat (and at `until`), and nobody can hold a machine from a Nexus that isn't running any more.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Claim { pub host: String, pub until: i64, pub note: String }

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct PSession { pub host: String, pub user: String, pub tmux: String, pub preset: String, pub name: String, pub cwd: String }

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Presence { pub client: String, pub name: String, pub login: String, pub status: String, pub ts: i64, pub claims: Vec<Claim>, pub sessions: Vec<PSession> }

fn preset_ok(p: &str) -> bool { (1..=24).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') }

/// Sanitise a presence record: ours before it is written, everyone's after it is read (others' presence is untrusted).
pub fn make_presence(p: &Value) -> Presence {
    let list = |k: &str| p[k].as_array().cloned().unwrap_or_default();
    Presence {
        client: clean(vstr(&p["client"]), 24),
        name: clean(vstr(&p["name"]), 40),
        login: clean(vstr(&p["login"]), 80),
        status: clean(vstr(&p["status"]), 140),
        ts: now_ms(),
        claims: list("claims").iter().take(10)
            .map(|c| Claim { host: clean(vstr(&c["host"]), 64), until: c["until"].as_f64().unwrap_or(0.0) as i64, note: clean(vstr(&c["note"]), 120) })
            .filter(|c| !c.host.is_empty() && c.until > 0).collect(),
        sessions: list("sessions").iter().take(40).map(|s| PSession {
            host: clean(vstr(&s["host"]), 64), user: clean(vstr(&s["user"]), 32), tmux: clean(vstr(&s["tmux"]), 64),
            // Keep the preset to a plain id (it picks an icon and a lookup key in the UI).
            preset: if preset_ok(vstr(&s["preset"])) { vstr(&s["preset"]).into() } else { "shell".into() },
            name: clean(vstr(&s["name"]), 60), cwd: clean(vstr(&s["cwd"]), 200),
        }).collect(),
    }
}

pub fn norm_dir(d: &str) -> &str { let t = d.trim_end_matches('/'); if t.is_empty() { "~" } else { t } }
pub const AGENT_PRESETS: &[&str] = &["claude", "antigravity", "openclaw"];

#[derive(Default, Debug)]
pub struct Conflicts { pub same_session: Vec<(String, PSession)>, pub same_folder: Vec<(String, PSession)> }

/// What would collide if *client* opened a session on host/user (a tmux session to attach, or a folder to start in)?
///   same_session: other people attached to that exact tmux session
///   same_folder:  other people running an agent in the same folder on the same machine/account
pub fn conflicts(presence: &[Presence], client: &str, host: &str, user: &str, tmux: Option<&str>, cwd: Option<&str>) -> Conflicts {
    let mut out = Conflicts::default();
    for p in presence.iter().filter(|p| p.client != client) {
        for s in p.sessions.iter().filter(|s| s.host == host && s.user == user) {
            match (tmux, cwd) {
                (Some(t), _) if !t.is_empty() => { if s.tmux == t { out.same_session.push((p.name.clone(), s.clone())); } }
                (_, Some(c)) if !c.is_empty() && AGENT_PRESETS.contains(&s.preset.as_str()) && norm_dir(&s.cwd) == norm_dir(c) => out.same_folder.push((p.name.clone(), s.clone())),
                _ => {}
            }
        }
    }
    out
}

// ---------------------------------------------------------------- remote commands
/// Keep the chat log bounded (last 2000 messages) and drop week-old presence files.
pub fn prepare_cmd() -> String {
    format!("mkdir -p {DIR}/presence && touch {DIR}/chat.jsonl && if [ $(wc -c < {DIR}/chat.jsonl) -gt 2000000 ]; then tail -n 2000 {DIR}/chat.jsonl > {DIR}/chat.tmp && mv -f {DIR}/chat.tmp {DIR}/chat.jsonl; fi; find {DIR}/presence -name '*.json' -mmin +10080 -delete 2>/dev/null; true")
}
pub fn tail_cmd() -> String { format!("exec tail -n 300 -F {DIR}/chat.jsonl 2>/dev/null") }
pub fn post_cmd() -> String { format!("cat >> {DIR}/chat.jsonl") }
/// Write our heartbeat and print everyone's fresh presence (including ours), one JSON document per line.
pub fn heartbeat_cmd(client: &str) -> String {
    let id: String = client.chars().filter(char::is_ascii_alphanumeric).collect(); // safe as a bare path segment
    let f = format!("{DIR}/presence/{}.json", if id.is_empty() { "anon" } else { &id });
    format!("mkdir -p {DIR}/presence && cat > {f}.tmp && mv -f {f}.tmp {f} && find {DIR}/presence -name '*.json' -mmin -{PRESENCE_TTL_MIN} -exec sh -c 'cat \"$1\"; echo' _ {{}} \\;")
}
pub fn parse_presence_list(stdout: &str) -> Vec<Presence> {
    stdout.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|p| !vstr(&p["client"]).is_empty()).map(|p| make_presence(&p)).collect()
}
pub const PRESETS_GET_CMD: &str = "cat ~/.nexus/collab/presets.json 2>/dev/null || echo \"[]\"";
pub const PRESETS_PUT_CMD: &str = "mkdir -p ~/.nexus/collab && cat > ~/.nexus/collab/presets.json.tmp && mv -f ~/.nexus/collab/presets.json.tmp ~/.nexus/collab/presets.json";

/// Splits a byte stream into chat messages (the hub's `tail -F` output).
#[derive(Default)]
pub struct LineBuf(Vec<u8>);
impl LineBuf {
    pub fn push(&mut self, data: &[u8]) -> Vec<Message> {
        self.0.extend_from_slice(data);
        let mut out = Vec::new();
        while let Some(i) = self.0.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.0.drain(..=i).collect();
            if let Some(m) = parse_line(String::from_utf8_lossy(&line[..i]).trim_end_matches('\r')) { out.push(m); }
        }
        if self.0.len() > 1 << 20 { self.0.clear(); }
        out
    }
}

/// "…" when a display string was cut.
pub fn ellipsize(s: &str, n: usize) -> String { if s.chars().count() > n { format!("{}…", clip(s, n)) } else { s.to_string() } }

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn messages_are_single_line_capped_and_clean() {
        let from = From { name: "Alex".into(), login: "alex@x.com".into(), client: "c1".into() };
        let m = make_message(&from, &format!("hi\u{1b}[2Jthere\nsecond line{}", "x".repeat(5000)), "msg");
        let line = serde_json::to_string(&m).unwrap();
        assert!(!line.contains('\n'), "serialized message has no raw newline");
        assert!(line.len() < 4096, "fits in one atomic append");
        assert!(!m.text.contains('\u{1b}'), "escape sequences removed");
        assert!(m.text.contains('\n'), "newlines inside the text are kept (escaped in JSON)");
        assert!(m.text.chars().count() <= MAX_TEXT);
        assert_eq!(m.kind, "msg");
        assert_eq!(make_message(&From::default(), "x", "evil").kind, "msg");
        assert_eq!(make_message(&From::default(), "x", "msg").from.name, "someone");
    }
    #[test]
    fn parse_line_round_trips_and_rejects_junk() {
        let m = make_message(&From { name: "Sam".into(), ..Default::default() }, "GPU free after 3pm", "status");
        let back = parse_line(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!((back.id.as_str(), back.ts, back.text.as_str(), back.kind.as_str()), (m.id.as_str(), m.ts, "GPU free after 3pm", "status"));
        assert!(parse_line("not json").is_none());
        assert!(parse_line("{\"id\":1}").is_none());
        assert!(parse_line("").is_none());
        let mut lb = LineBuf::default();
        let line = serde_json::to_string(&m).unwrap();
        assert!(lb.push(line[..10].as_bytes()).is_empty());
        assert_eq!(lb.push(format!("{}\njunk\n", &line[10..]).as_bytes()).len(), 1);
    }
    #[test]
    fn conflicts_same_session_same_agent_folder() {
        let sess = |host: &str, user: &str, tmux: &str, preset: &str, cwd: &str| json!({ "host": host, "user": user, "tmux": tmux, "preset": preset, "cwd": cwd });
        let presence = vec![
            make_presence(&json!({ "client": "me", "name": "Me", "sessions": [sess("spark1-aresj", "jcee", "nx-claude-mine", "claude", "~/ARES-J")] })),
            make_presence(&json!({ "client": "a", "name": "Alex", "sessions": [sess("spark1-aresj", "jcee", "nx-claude-1", "claude", "~/ARES-J/"), sess("spark1-aresj", "jcee", "nx-shell-2", "shell", "~/notes")] })),
            make_presence(&json!({ "client": "b", "name": "Bea", "sessions": [sess("spark2-aresj", "jcee-slave", "nx-claude-1", "claude", "~/ARES-J")] })),
        ];
        let who = |v: &[(String, PSession)]| v.iter().map(|x| x.0.clone()).collect::<Vec<_>>();
        assert_eq!(who(&conflicts(&presence, "me", "spark1-aresj", "jcee", Some("nx-claude-1"), None).same_session), ["Alex"]);
        assert_eq!(who(&conflicts(&presence, "me", "spark1-aresj", "jcee", None, Some("~/ARES-J")).same_folder), ["Alex"]);
        // a shell in the same folder is not an agent conflict
        assert!(conflicts(&presence, "me", "spark1-aresj", "jcee", None, Some("~/notes")).same_folder.is_empty());
        // my own sessions never conflict with me
        assert!(conflicts(&presence, "me", "spark1-aresj", "jcee", Some("nx-claude-mine"), None).same_session.is_empty());
    }
    #[test]
    fn untrusted_presence_is_reduced() {
        let p = make_presence(&json!({ "client": "x", "sessions": [{ "preset": "__proto__" }, { "preset": "<img src=x>" }, { "preset": "claude" }],
            "claims": [{ "host": "gpu-box", "until": 5000, "note": "x" }, { "host": "", "until": 5000 }, { "host": "h", "until": "soon" }] }));
        assert_eq!(p.sessions.iter().map(|s| s.preset.as_str()).collect::<Vec<_>>(), ["shell", "shell", "claude"]);
        assert_eq!(p.claims.len(), 1);
        let list = parse_presence_list(&format!("{}\nnot json\n{{\"name\":\"no client\"}}\n", serde_json::to_string(&p).unwrap()));
        assert_eq!(list.len(), 1);
        assert!(heartbeat_cmd("../../etc/x").contains("presence/etcx.json"));
    }
}
