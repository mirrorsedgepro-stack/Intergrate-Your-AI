//! Persistent settings: machines, presets, appearance. Stored as config.json in the app data folder
//! (%APPDATA%\Nexus, ~/Library/Application Support/Nexus, ~/.config/Nexus): the same file the Electron builds used,
//! so an upgrade keeps every machine, preset, workspace and pinned host key.
use crate::platform::platform_os;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn is_false(b: &bool) -> bool { !*b }
fn d_true() -> bool { true }
fn lenient_port<'de, D: Deserializer<'de>>(d: D) -> Result<u16, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Number(n) => n.as_u64().unwrap_or(22) as u16,
        Value::String(s) => s.trim().parse().unwrap_or(22),
        _ => 22,
    })
}
fn lenient_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match Value::deserialize(d)? { Value::String(s) => s, Value::Null => String::new(), v => v.to_string() })
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Appearance {
    pub font_size: f32,
    pub font_family: String,
    pub line_height: f32,
    pub copy_on_select: bool,
    pub notify_when_done: bool,
    pub restore_session: bool,
    pub record_sessions: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
impl Default for Appearance {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            font_family: r#""JetBrains Mono", "Cascadia Code", Menlo, Consolas, monospace"#.into(),
            line_height: 1.15,
            copy_on_select: false,
            notify_when_done: true,
            restore_session: true,
            record_sessions: false,
            extra: Map::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub color: String,
    #[serde(skip_serializing_if = "String::is_empty", deserialize_with = "lenient_string")]
    pub icon: String,
    pub cmd: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only: Option<String>,
}
impl Preset {
    fn new(id: &str, name: &str, color: &str, win: &str, linux: Option<&str>, only: Option<&str>) -> Self {
        let mut cmd = BTreeMap::new();
        cmd.insert("windows".to_string(), win.to_string());
        if let Some(l) = linux { cmd.insert("linux".to_string(), l.to_string()); }
        Self { id: id.into(), name: name.into(), color: color.into(), icon: String::new(), cmd, only: only.map(Into::into) }
    }
    /// The command this preset runs on a machine with the given OS ("" = plain shell).
    pub fn cmd_for(&self, os: &str) -> String {
        match self.cmd.get(os) {
            Some(c) => c.clone(),
            None if os == "mac" => self.cmd.get("linux").cloned().unwrap_or_default(),
            None => String::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Machine {
    pub id: String,
    pub name: String,
    /// local | ssh | wsl
    #[serde(rename = "type")]
    pub kind: String,
    pub os: String,
    #[serde(skip_serializing_if = "String::is_empty", deserialize_with = "lenient_string")]
    pub host: String,
    #[serde(deserialize_with = "lenient_port", skip_serializing_if = "port_unset")]
    pub port: u16,
    #[serde(skip_serializing_if = "String::is_empty", deserialize_with = "lenient_string")]
    pub user: String,
    #[serde(skip_serializing_if = "String::is_empty", deserialize_with = "lenient_string")]
    pub key_path: String,
    pub color: String,
    #[serde(skip_serializing_if = "String::is_empty", deserialize_with = "lenient_string")]
    pub icon: String,
    #[serde(skip_serializing_if = "String::is_empty", deserialize_with = "lenient_string")]
    pub ts_host: String,
    #[serde(skip_serializing_if = "String::is_empty", deserialize_with = "lenient_string")]
    pub jump: String,
    #[serde(skip_serializing_if = "String::is_empty", deserialize_with = "lenient_string")]
    pub distro: String,
    #[serde(skip_serializing_if = "is_false")]
    pub invited: bool,
    /// A key passphrase someone put in config.json by hand. Honoured, never shown or edited by the UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
fn port_unset(p: &u16) -> bool { *p == 0 }
impl Machine {
    pub fn is_ssh(&self) -> bool { self.kind == "ssh" }
    pub fn is_local(&self) -> bool { self.kind == "local" }
    pub fn is_wsl(&self) -> bool { self.kind == "wsl" }
    pub fn port_or_default(&self) -> u16 { if self.port == 0 { 22 } else { self.port } }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Split { pub x: f32, pub y: f32 }
impl Default for Split { fn default() -> Self { Self { x: 0.5, y: 0.5 } } }

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct SnapSession {
    pub machine_id: String,
    pub preset_id: String,
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmux_name: Option<String>,
    pub name: Option<String>,
}
/// A saved arrangement: the last session (restored on start) or a named workspace.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Snapshot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub layout: u8,
    pub split: Split,
    pub focus: usize,
    pub sessions: Vec<SnapSession>,
    /// Pane index -> index into `sessions` (-1 = empty).
    pub panes: Vec<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct WindowCfg { pub x: Option<f32>, pub y: Option<f32>, pub width: Option<f32>, pub height: Option<f32>, pub maximized: bool }

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct TailscaleCfg {
    /// Electron-era token (encrypted by Chromium's safeStorage; unreadable here). Kept so a downgrade still works.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_guest_users: Option<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct CollabCfg {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub client: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hub: String,
    #[serde(default = "d_true")]
    pub announce: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub status: String,
}
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct McpCfg { pub enabled: bool }

/// A saved prompt (the prompt library): picked from the palette, it fills the broadcast composer.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct SavedPrompt { pub name: String, pub text: String }

/// Phone alerts: an ntfy topic address (https://ntfy.sh/<topic> or your own server) that is told when an agent
/// finishes or needs you while Nexus isn't in front.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct PushCfg {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// Also send what the agent said (off: only which session is waiting).
    pub include_text: bool,
}
impl PushCfg { pub fn is_default(&self) -> bool { *self == Self::default() } }

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub version: u32,
    pub appearance: Appearance,
    pub offline: bool,
    pub mcp: McpCfg,
    pub presets: Vec<Preset>,
    pub machines: Vec<Machine>,
    pub known_hosts: BTreeMap<String, String>,
    pub workspaces: Vec<Snapshot>,
    pub recent_dirs: BTreeMap<String, Vec<String>>,
    pub last_session: Option<Snapshot>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub presets_added: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub wsl_seen: BTreeMap<String, bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowCfg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tailscale: Option<TailscaleCfg>,
    pub collab: CollabCfg,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<SavedPrompt>,
    #[serde(skip_serializing_if = "PushCfg::is_default")]
    pub push: PushCfg,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

pub fn default_presets() -> Vec<Preset> {
    vec![
        Preset::new("claude", "Claude", "#E08A62", "claude", Some("claude"), None),
        Preset::new("antigravity", "Antigravity", "#7BA7FF", "agy", Some("agy"), None),
        Preset::new("openclaw", "OpenClaw", "#FF5C5C", "openclaw tui", Some("openclaw tui"), None),
        Preset::new("shell", "Shell", "#73DACA", "", Some(""), None),
        Preset::new("gitbash", "Git Bash", "#E0AF68", "__gitbash__", None, Some("windows")),
    ]
}

impl Default for Config {
    fn default() -> Self {
        let os = platform_os();
        Self {
            version: 1,
            appearance: Appearance::default(),
            offline: false,
            mcp: McpCfg::default(),
            presets: default_presets(),
            machines: vec![
                Machine { id: "local".into(), name: if os == "mac" { "This Mac" } else { "This PC" }.into(), kind: "local".into(), os: os.into(), color: "#7AA2F7".into(), ..Default::default() },
                Machine { id: "linux".into(), name: "Linux box".into(), kind: "ssh".into(), os: "linux".into(), port: 22, color: "#BB9AF7".into(), ..Default::default() },
            ],
            known_hosts: BTreeMap::new(),
            workspaces: vec![],
            recent_dirs: BTreeMap::new(),
            last_session: None,
            presets_added: vec![],
            wsl_seen: BTreeMap::new(),
            window: None,
            tailscale: None,
            collab: CollabCfg { announce: true, ..Default::default() },
            prompts: vec![],
            push: PushCfg::default(),
            extra: Map::new(),
        }
    }
}

/// Built-in presets added after the first release. Each is added to existing configs once (recorded in
/// presetsAdded), so deleting one in the config file sticks.
pub const ADDED_PRESETS: &[&str] = &["openclaw"];

impl Config {
    /// Bring an older config up to date in place. Returns true if anything changed.
    pub fn migrate(&mut self) -> bool {
        let mut changed = false;
        let defaults = default_presets();
        // Gemini CLI became the Antigravity CLI (`agy`): swap the old default preset in place.
        if let Some(i) = self.presets.iter().position(|p| p.id == "gemini" && p.cmd.get("linux").is_some_and(|c| c == "gemini")) {
            if !self.presets.iter().any(|p| p.id == "antigravity") {
                self.presets[i] = defaults.iter().find(|p| p.id == "antigravity").cloned().unwrap_or_default();
                changed = true;
            }
        }
        for id in ADDED_PRESETS {
            if self.presets_added.iter().any(|x| x == id) { continue; }
            self.presets_added.push((*id).to_string());
            changed = true;
            if self.presets.iter().any(|p| p.id == *id) { continue; }
            // Agents go before the shells, after the last built-in agent the user still has.
            let at = self.presets.iter().position(|p| p.id == "shell").unwrap_or(self.presets.len());
            if let Some(p) = defaults.iter().find(|p| p.id == *id) { self.presets.insert(at, p.clone()); }
        }
        changed
    }
    pub fn machine(&self, id: &str) -> Option<&Machine> { self.machines.iter().find(|m| m.id == id) }
    pub fn preset(&self, id: &str) -> Option<&Preset> { self.presets.iter().find(|p| p.id == id) }
    pub fn presets_for<'a>(&'a self, m: &'a Machine) -> impl Iterator<Item = &'a Preset> {
        self.presets.iter().filter(move |p| p.only.as_deref().is_none_or(|o| o == m.os))
    }
    /// Parse a config.json: defaults fill whatever the file lacks (arrays in the file replace the defaults).
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> { serde_json::from_str(text) }
}

pub fn dir() -> PathBuf {
    if let Some(d) = std::env::var_os("NEXUS_CONFIG_DIR") { return PathBuf::from(d); }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("Nexus")
}
pub fn file() -> PathBuf { dir().join("config.json") }

pub fn load() -> Config {
    let path = file();
    let mut cfg = match std::fs::read_to_string(&path) {
        Ok(text) => match Config::from_json(&text) {
            Ok(mut c) => { if c.migrate() { save(&c); } c }
            Err(_) => {
                // Unreadable (hand-edited badly?): keep the user's file beside the fresh one instead of losing it.
                let _ = std::fs::copy(&path, path.with_extension("json.bad"));
                let mut c = Config::default();
                c.migrate();
                save(&c);
                c
            }
        },
        Err(_) => { let mut c = Config::default(); c.migrate(); save(&c); c }
    };
    // The local machine is always this computer, whatever OS the config was written on.
    for m in &mut cfg.machines { if m.is_local() { m.os = platform_os().into(); } }
    cfg
}

pub fn save(cfg: &Config) {
    let path = file();
    if let Some(d) = path.parent() { let _ = std::fs::create_dir_all(d); }
    let Ok(text) = serde_json::to_string_pretty(cfg) else { return };
    // Write beside and rename, so a crash mid-write can't leave half a config.
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() { let _ = std::fs::rename(&tmp, &path); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_swaps_gemini_for_antigravity_once() {
        let mut cfg = Config::default();
        cfg.presets.retain(|p| p.id != "antigravity");
        cfg.presets.insert(1, Preset::new("gemini", "Gemini", "", "gemini", Some("gemini"), None));
        assert!(cfg.migrate());
        assert_eq!(cfg.presets[1].id, "antigravity");
        assert_eq!(cfg.presets[1].cmd["linux"], "agy");
        assert!(!cfg.migrate());
    }
    #[test]
    fn migrate_adds_openclaw_once_before_the_shells() {
        // A v1.3.0 config: saved presets without OpenClaw (arrays in the file replace the defaults).
        let old: Vec<Preset> = default_presets().into_iter().filter(|p| p.id != "openclaw").collect();
        let mut cfg = Config::from_json(&serde_json::json!({ "presets": old }).to_string()).unwrap();
        assert!(cfg.migrate());
        let ids: Vec<&str> = cfg.presets.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(&ids[..4], ["claude", "antigravity", "openclaw", "shell"]);
        assert_eq!(cfg.presets[2].cmd["linux"], "openclaw tui");
        assert!(!cfg.migrate());
        // Deleted by the user afterwards: it stays deleted.
        cfg.presets.retain(|p| p.id != "openclaw");
        assert!(!cfg.migrate());
        assert!(!cfg.presets.iter().any(|p| p.id == "openclaw"));
        // A fresh config already has it and is only marked.
        let mut fresh = Config::default();
        fresh.migrate();
        assert_eq!(fresh.presets.iter().filter(|p| p.id == "openclaw").count(), 1);
    }
    #[test]
    fn electron_era_config_round_trips_with_unknown_keys() {
        let text = r##"{"version":1,"appearance":{"fontSize":16,"theme":"x"},"machines":[{"id":"linux","name":"Spark","type":"ssh","os":"linux","host":"10.0.0.5","port":"2222","user":"me","keyPath":"","color":"#BB9AF7","hostname":"spark1","passphrase":"s3"}],
          "knownHosts":{"10.0.0.5:2222":"SHA256:abc"},"lastSession":null,"tailscale":{"token":"AAAA","perGuestUsers":true},"future":{"a":1},
          "workspaces":[{"name":"w","layout":2,"split":{"x":0.4,"y":0.5},"focus":1,"sessions":[{"machineId":"linux","presetId":"claude","cwd":"~/p","name":"Claude · p"}],"panes":[0,-1]}]}"##;
        let cfg = Config::from_json(text).unwrap();
        assert_eq!(cfg.appearance.font_size, 16.0);
        assert_eq!(cfg.appearance.line_height, 1.15);
        assert_eq!(cfg.machines[0].port, 2222);
        assert_eq!(cfg.machines[0].passphrase.as_deref(), Some("s3"));
        assert_eq!(cfg.presets.len(), 5, "defaults fill a missing presets array");
        assert_eq!(cfg.workspaces[0].panes, vec![0, -1]);
        let back: Value = serde_json::to_value(&cfg).unwrap();
        assert_eq!(back["future"]["a"], 1);
        assert_eq!(back["appearance"]["theme"], "x");
        assert_eq!(back["machines"][0]["hostname"], "spark1");
        assert_eq!(back["machines"][0]["type"], "ssh");
        assert_eq!(back["tailscale"]["token"], "AAAA");
        assert_eq!(back["knownHosts"]["10.0.0.5:2222"], "SHA256:abc");
        assert_eq!(Config::from_json(&back.to_string()).unwrap(), cfg);
    }
    #[test]
    fn preset_commands_fall_back_per_os() {
        let p = Preset::new("claude", "Claude", "", "claude.cmd", Some("claude"), None);
        assert_eq!(p.cmd_for("windows"), "claude.cmd");
        assert_eq!(p.cmd_for("mac"), "claude");
        assert_eq!(Preset::new("gitbash", "", "", "__gitbash__", None, None).cmd_for("linux"), "");
    }
}
