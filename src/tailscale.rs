//! Tailscale integration: the local client (status/peers), the Tailscale API (invites, policy, tags), and the
//! pure policy/invite-code helpers. Guests are invited to the owner's tailnet and reach only the tagged Sparks,
//! over Tailscale SSH, as the configured Linux users.
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const API: &str = "https://api.tailscale.com/api/v2";
pub const GUEST_GROUP: &str = "group:nexus-guests";
pub const SPARK_TAG: &str = "tag:nexus-spark";
const INVITE_PREFIX: &str = "nexus-invite:";
const B64URL: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

// ---------------------------------------------------------------- local client
fn cli_path() -> String {
    #[cfg(target_os = "macos")]
    { let app = "/Applications/Tailscale.app/Contents/MacOS/Tailscale"; if std::path::Path::new(app).exists() { return app.into(); } }
    #[cfg(windows)]
    {
        let p = std::path::PathBuf::from(std::env::var_os("ProgramFiles").unwrap_or_else(|| "C:\\Program Files".into())).join("Tailscale").join("tailscale.exe");
        if p.exists() { return p.to_string_lossy().into_owned(); }
    }
    "tailscale".into()
}

#[derive(Clone, Debug, Default)]
pub struct Peer { pub host_name: String, pub dns_name: String, pub ips: Vec<String>, pub online: bool }
#[derive(Clone, Debug, Default)]
pub struct TsUser { pub login: String, pub name: String }
#[derive(Clone, Debug, Default)]
pub struct LocalStatus {
    pub installed: bool,
    pub running: bool,
    /// Running | NeedsLogin | Stopped …
    pub state: String,
    pub tailnet: Option<String>,
    pub self_peer: Option<Peer>,
    /// The signed-in Tailscale user (used as the default name in team chat).
    pub user: Option<TsUser>,
    pub peers: Vec<Peer>,
}

fn s(v: &Value) -> String { v.as_str().unwrap_or_default().to_string() }
fn strs(v: &Value) -> Vec<String> { v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default() }

pub fn parse_local_status(data: &Value) -> LocalStatus {
    let peer = |p: &Value| Peer { host_name: s(&p["HostName"]), dns_name: s(&p["DNSName"]).trim_end_matches('.').to_string(), ips: strs(&p["TailscaleIPs"]), online: p["Online"].as_bool().unwrap_or(false) };
    let me = &data["Self"];
    let user = me["UserID"].as_i64().map(|id| &data["User"][id.to_string()]).filter(|u| u.is_object()).map(|u| TsUser { login: s(&u["LoginName"]), name: s(&u["DisplayName"]) });
    LocalStatus {
        installed: true,
        running: true,
        state: s(&data["BackendState"]),
        tailnet: data["CurrentTailnet"]["Name"].as_str().map(String::from),
        self_peer: me.is_object().then(|| peer(me)),
        user,
        peers: data["Peer"].as_object().map(|m| m.values().map(peer).collect()).unwrap_or_default(),
    }
}

/// Blocking: runs `tailscale status --json`.
pub fn local_status() -> LocalStatus {
    let mut c = crate::platform::command(cli_path());
    c.args(["status", "--json"]);
    let o = crate::platform::run(c, None, Duration::from_secs(8));
    if o.missing { return LocalStatus::default(); }
    match serde_json::from_slice::<Value>(&o.stdout) {
        Ok(v) => parse_local_status(&v),
        Err(_) => LocalStatus { installed: true, ..Default::default() },
    }
}

// ---------------------------------------------------------------- credentials
// Two kinds of credential work with the API:
//   tskey-api-…                     a personal API access token: carries the owner's full admin rights, expires ≤ 90 days.
//   [client_id:]tskey-client-…      an OAuth client secret: scoped to what Nexus needs (devices:core on tag:nexus-spark,
//                                   policy_file, users), never expires, and is exchanged for 1-hour access tokens.
// The OAuth client is the recommended choice: a leaked credential then gives an attacker Nexus's permissions, not yours.
pub const OAUTH_SCOPES: &[&str] = &["devices:core", "devices:core:read", "devices:posture_attributes", "policy_file", "users"];

#[derive(Clone, Debug, PartialEq)]
pub enum Credential { Api { token: String }, OAuth { client_id: Option<String>, secret: String } }
impl Credential { pub fn kind(&self) -> &'static str { if matches!(self, Self::Api { .. }) { "api" } else { "oauth" } } }

fn wordy(s: &str) -> bool { !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') }
pub fn parse_credential(raw: &str) -> Option<Credential> {
    let t = raw.trim();
    if t.strip_prefix("tskey-api-").is_some_and(wordy) { return Some(Credential::Api { token: t.into() }); }
    let (id, secret) = match t.split_once(':') { Some((id, sec)) if wordy(id) => (Some(id.to_string()), sec), Some(_) => return None, None => (None, t) };
    secret.strip_prefix("tskey-client-").is_some_and(wordy).then(|| Credential::OAuth { client_id: id, secret: secret.into() })
}

// ---------------------------------------------------------------- API client
pub struct HttpReq { pub method: &'static str, pub url: String, pub headers: Vec<(String, String)>, pub body: Option<String> }
pub struct HttpRes { pub status: u16, pub body: String, pub etag: Option<String> }
pub type Http = Arc<dyn Fn(HttpReq) -> Result<HttpRes, String> + Send + Sync>;

/// The real transport (blocking, rustls).
pub fn ureq_http() -> Http {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(30))).build().into();
    Arc::new(move |r: HttpReq| {
        let res = match (r.method, r.body) {
            ("GET", _) => { let mut q = agent.get(&r.url); for (k, v) in &r.headers { q = q.header(k, v); } q.call() }
            ("DELETE", _) => { let mut q = agent.delete(&r.url); for (k, v) in &r.headers { q = q.header(k, v); } q.call() }
            (_, body) => { let mut q = agent.post(&r.url); for (k, v) in &r.headers { q = q.header(k, v); } match body { Some(b) => q.send(b), None => q.send_empty() } }
        };
        let mut res = res.map_err(|e| format!("Can't reach Tailscale: {e}"))?;
        let etag = res.headers().get("etag").and_then(|v| v.to_str().ok()).map(String::from);
        let status = res.status().as_u16();
        let body = res.body_mut().read_to_string().unwrap_or_default();
        Ok(HttpRes { status, body, etag })
    })
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

pub struct TailscaleApi {
    cred: Credential,
    http: Http,
    /// (token, expiry ms) for OAuth clients.
    access: Mutex<Option<(String, i64)>>,
}

impl TailscaleApi {
    pub fn new(cred: Credential, http: Http) -> Self { Self { cred, http, access: Mutex::new(None) } }

    /// Bearer token for the next request: the API token itself, or a fresh OAuth access token (renewed 2 min early).
    fn bearer(&self) -> Result<String, String> {
        let (client_id, secret) = match &self.cred { Credential::Api { token } => return Ok(token.clone()), Credential::OAuth { client_id, secret } => (client_id, secret) };
        let now = crate::util::now_ms();
        if let Some((tok, exp)) = self.access.lock().unwrap().as_ref() { if exp - now > 120_000 { return Ok(tok.clone()); } }
        let mut form = format!("client_secret={}", enc(secret));
        if let Some(id) = client_id { form.push_str(&format!("&client_id={}", enc(id))); }
        let res = (self.http)(HttpReq { method: "POST", url: format!("{API}/oauth/token"), headers: vec![("Content-Type".into(), "application/x-www-form-urlencoded".into())], body: Some(form) })?;
        let json: Value = serde_json::from_str(&res.body).unwrap_or(Value::Null);
        let Some(tok) = json["access_token"].as_str().filter(|_| res.status < 300) else {
            let msg = ["message", "error_description", "error"].iter().find_map(|k| json[*k].as_str()).map(String::from).unwrap_or(res.body);
            return Err(format!("Tailscale rejected the OAuth client ({}): {msg}. Check the client ID/secret and that it has the scopes {}", res.status, OAUTH_SCOPES.join(", ")));
        };
        *self.access.lock().unwrap() = Some((tok.to_string(), now + json["expires_in"].as_i64().unwrap_or(3600) * 1000));
        Ok(tok.to_string())
    }

    fn req(&self, method: &'static str, path: &str, body: Option<&Value>, extra: &[(&str, String)]) -> Result<(Value, Option<String>), String> {
        let mut headers = vec![("Authorization".to_string(), format!("Bearer {}", self.bearer()?))];
        if body.is_some() { headers.push(("Content-Type".into(), "application/json".into())); }
        headers.extend(extra.iter().map(|(k, v)| ((*k).to_string(), v.clone())));
        let res = (self.http)(HttpReq { method, url: format!("{API}{path}"), headers, body: body.map(Value::to_string) })?;
        let json: Value = serde_json::from_str(&res.body).unwrap_or(Value::Null);
        if res.status >= 300 {
            let msg = json["message"].as_str().map(String::from).or_else(|| json.get("data").map(Value::to_string)).unwrap_or(res.body);
            return Err(format!("Tailscale API {}: {msg}", res.status));
        }
        Ok((json, res.etag))
    }
    fn list(&self, path: &str, key: Option<&str>) -> Result<Vec<Value>, String> {
        let (v, _) = self.req("GET", &format!("/tailnet/-{path}"), None, &[])?;
        Ok(match key { Some(k) => v[k].as_array(), None => v.as_array() }.cloned().unwrap_or_default())
    }
    pub fn devices(&self) -> Result<Vec<Value>, String> { self.list("/devices", Some("devices")) }
    pub fn users(&self) -> Result<Vec<Value>, String> { self.list("/users?type=all", Some("users")) }
    pub fn invites(&self) -> Result<Vec<Value>, String> { self.list("/user-invites", None) }
    pub fn create_invite(&self, email: &str) -> Result<Value, String> {
        let (v, _) = self.req("POST", "/tailnet/-/user-invites", Some(&json!([{ "role": "member", "email": email }])), &[])?;
        Ok(v[0].clone())
    }
    pub fn delete_invite(&self, id: &str) -> Result<(), String> { self.req("DELETE", &format!("/user-invites/{}", enc(id)), None, &[]).map(drop) }
    pub fn delete_user(&self, id: &str) -> Result<(), String> { self.req("POST", &format!("/users/{}/delete", enc(id)), None, &[]).map(drop) }
    pub fn set_tags(&self, device_id: &str, tags: &[String]) -> Result<(), String> {
        self.req("POST", &format!("/device/{}/tags", enc(device_id)), Some(&json!({ "tags": tags })), &[]).map(drop)
    }
    pub fn get_policy(&self) -> Result<(Value, Option<String>), String> {
        let (v, etag) = self.req("GET", "/tailnet/-/acl", None, &[("Accept", "application/json".into())])?;
        Ok((if v.is_object() { v } else { json!({}) }, etag))
    }
    pub fn validate_policy(&self, policy: &Value) -> Result<Value, String> { Ok(self.req("POST", "/tailnet/-/acl/validate", Some(policy), &[])?.0) }
    pub fn set_policy(&self, policy: &Value, etag: Option<&str>) -> Result<(), String> {
        let mut h = vec![("Accept", "application/json".to_string())];
        if let Some(e) = etag { h.push(("If-Match", e.to_string())); }
        self.req("POST", "/tailnet/-/acl", Some(policy), &h).map(drop)
    }
}

// ---------------------------------------------------------------- policy planning (pure)
fn has(list: &Value, v: &str) -> bool { list.as_array().is_some_and(|a| a.iter().any(|x| x == v)) }
fn is_allow_all_src(src: &Value) -> bool { has(src, "*") || has(src, "autogroup:member") }
fn same_set<S: AsRef<str>>(a: &Value, b: &[S]) -> bool {
    let mut x: Vec<String> = a.as_array().map(|v| v.iter().map(|e| e.as_str().map_or_else(|| e.to_string(), String::from)).collect()).unwrap_or_default();
    let mut y: Vec<String> = b.iter().map(|e| e.as_ref().to_string()).collect();
    x.sort();
    y.sort();
    x == y
}
fn obj<'a>(p: &'a mut Value, key: &str) -> &'a mut Map<String, Value> {
    if !p[key].is_object() { p[key] = json!({}); }
    p[key].as_object_mut().unwrap()
}
fn arr<'a>(p: &'a mut Value, key: &str) -> &'a mut Vec<Value> {
    if !p[key].is_array() { p[key] = json!([]); }
    p[key].as_array_mut().unwrap()
}

#[derive(Default, Clone)]
pub struct PlanOpts { pub guests: Vec<String>, pub ssh_users: Vec<String>, pub per_guest_users: bool }

/// Returns (policy, changes) where policy lets the owner (admins) keep full access, gives guests SSH on the tagged
/// Sparks only (as ssh_users), and turns any "everyone can reach everything" rule into an admins-only rule so that
/// invited guests can't reach the owner's other devices. Idempotent: planning twice yields no further changes.
pub fn plan_policy(current: &Value, o: &PlanOpts) -> (Value, Vec<String>) {
    let mut p = if current.is_object() { current.clone() } else { json!({}) };
    let mut changes = Vec::new();

    let mut wanted: Vec<String> = o.guests.iter().map(|g| g.trim().to_lowercase()).filter(|g| !g.is_empty()).collect();
    wanted.sort();
    wanted.dedup();
    let groups = obj(&mut p, "groups");
    if !same_set(groups.get(GUEST_GROUP).unwrap_or(&Value::Null), &wanted) {
        changes.push(format!("Guest group {GUEST_GROUP}: {}", if wanted.is_empty() { "(no guests yet)".into() } else { wanted.join(", ") }));
        groups.insert(GUEST_GROUP.into(), json!(wanted));
    }

    let owners = obj(&mut p, "tagOwners");
    if !same_set(owners.get(SPARK_TAG).unwrap_or(&Value::Null), &["autogroup:admin"]) {
        owners.insert(SPARK_TAG.into(), json!(["autogroup:admin"]));
        changes.push(format!("Create tag {SPARK_TAG} (owned by admins) for the Sparks"));
    }

    // Narrow every rule whose source is "everyone" (legacy "acls" and "grants") to admins. Guests become members of the
    // tailnet, so any rule with src * / autogroup:member reaches them: not only "*:*", but also "*:22", "tag:prod:*" and
    // the like. The only everyone-rule that is safe to keep is "members reach their own devices" (autogroup:self).
    let mut narrowed = false;
    let self_only = |dst: &Value| dst.as_array().is_some_and(|d| !d.is_empty() && d.iter().all(|x| x.as_str().is_some_and(|s| s == "autogroup:self" || s.starts_with("autogroup:self:"))));
    let describe = |dst: &Value| { let v = strs(dst); if v.is_empty() { "(no destination)".to_string() } else { v.join(", ") } };
    if let Some(acls) = p.get_mut("acls").and_then(Value::as_array_mut) {
        for rule in acls {
            let src_key = if rule.get("src").is_some_and(|v| !v.is_null()) { "src" } else { "users" };
            let dst = rule.get("dst").filter(|v| !v.is_null()).or_else(|| rule.get("ports")).cloned().unwrap_or(json!([]));
            if rule["action"] == "accept" && is_allow_all_src(&rule[src_key]) && !self_only(&dst) {
                rule[src_key] = json!(["autogroup:admin"]);
                narrowed = true;
                changes.push(format!("Limit the \"everyone → {}\" rule to admins (you), so guests can't use it", describe(&dst)));
            }
        }
    }
    if let Some(grants) = p.get_mut("grants").and_then(Value::as_array_mut) {
        for g in grants {
            let dst = g.get("dst").cloned().unwrap_or(json!([]));
            if is_allow_all_src(&g["src"]) && !self_only(&dst) {
                g["src"] = json!(["autogroup:admin"]);
                narrowed = true;
                changes.push(format!("Limit the \"everyone → {}\" grant to admins (you), so guests can't use it", describe(&dst)));
            }
        }
    }

    let grants = arr(&mut p, "grants");
    let mut ensure = |src: &str, dst: &str, ip: &str, why: &str| {
        if grants.iter().any(|x| same_set(&x["src"], &[src]) && same_set(&x["dst"], &[dst]) && same_set(&x["ip"], &[ip])) { return; }
        grants.push(json!({ "src": [src], "dst": [dst], "ip": [ip] }));
        changes.push(why.to_string());
    };
    if narrowed { ensure("autogroup:member", "autogroup:self", "*", "Members can still reach their own devices"); }
    ensure("autogroup:admin", SPARK_TAG, "*", "Admins (you) keep full access to the Sparks");
    ensure(GUEST_GROUP, SPARK_TAG, "tcp:22", "Guests can reach the Sparks on SSH (port 22) only");

    let mut users: Vec<String> = o.ssh_users.iter().filter(|u| !u.is_empty()).cloned().collect();
    users.sort();
    users.dedup();
    let who = if users.is_empty() { "(no users)".to_string() } else { users.join(" / ") };
    // Shared-account mode (the original): one rule lets admins and guests in as the machines' configured users.
    // Per-guest mode: admins keep that rule for themselves; each guest gets a rule for their own Linux account only,
    // so a guest can't read the owner's files, agent sign-ins or tmux sessions, and removing them removes real access.
    let legacy_src = ["autogroup:admin", GUEST_GROUP];
    let shared_src: &[&str] = if o.per_guest_users { &["autogroup:admin"] } else { &legacy_src };
    let ssh = arr(&mut p, "ssh");
    if o.per_guest_users {
        if let Some(legacy) = ssh.iter_mut().find(|r| same_set(&r["src"], &legacy_src) && same_set(&r["dst"], &[SPARK_TAG])) {
            legacy["src"] = json!(["autogroup:admin"]);
            changes.push(format!("Tailscale SSH: guests no longer share your account ({}); each gets their own", users.join(" / ")));
        }
    }
    match ssh.iter_mut().find(|r| same_set(&r["src"], shared_src) && same_set(&r["dst"], &[SPARK_TAG])) {
        None => {
            ssh.push(json!({ "action": "accept", "src": shared_src, "dst": [SPARK_TAG], "users": users }));
            changes.push(if o.per_guest_users { format!("Tailscale SSH: you log in to the shared machines as {who}") } else { format!("Tailscale SSH: you and guests log in to the shared machines as {who}") });
        }
        Some(existing) if !same_set(&existing["users"], &users) || existing["action"] != "accept" => {
            existing["users"] = json!(users);
            existing["action"] = json!("accept");
            changes.push(format!("Tailscale SSH users on the shared machines: {}", users.join(" / ")));
        }
        Some(_) => {}
    }
    if o.per_guest_users {
        // One rule per guest: src [email] -> dst [tag] as [their account]. Rules for guests no longer in the group go.
        let is_guest_rule = |r: &Value| r["src"].as_array().is_some_and(|s| s.len() == 1 && s[0].as_str().is_some_and(|e| e.contains('@'))) && same_set(&r["dst"], &[SPARK_TAG]);
        let rule_email = |r: &Value| r["src"][0].as_str().unwrap_or_default().to_lowercase();
        for email in &wanted {
            let u = guest_user(email);
            match ssh.iter_mut().find(|x| is_guest_rule(x) && rule_email(x) == *email) {
                None => { ssh.push(json!({ "action": "accept", "src": [email], "dst": [SPARK_TAG], "users": [u] })); changes.push(format!("Tailscale SSH: {email} logs in to the shared machines as {u}")); }
                Some(r) if !same_set(&r["users"], &[&u]) || r["action"] != "accept" => { r["users"] = json!([u]); r["action"] = json!("accept"); changes.push(format!("Tailscale SSH: {email} logs in as {u}")); }
                Some(_) => {}
            }
        }
        let mut i = ssh.len();
        while i > 0 {
            i -= 1;
            if is_guest_rule(&ssh[i]) && !wanted.contains(&rule_email(&ssh[i])) {
                let r = ssh.remove(i);
                changes.push(format!("Tailscale SSH: remove {}'s login", r["src"][0].as_str().unwrap_or_default()));
            }
        }
    }
    (p, changes)
}

/// The Linux account a guest gets in per-guest mode: nexus-<local part of their email>, shortened and sanitised so it
/// is a valid user name everywhere (lower-case, [a-z0-9-], 32 chars). Collisions between e.g. a.b@ and a_b@ are
/// possible but harmless: Tailscale SSH still binds each account to one email.
pub fn guest_user(email: &str) -> String {
    let lower = email.to_lowercase();
    let mut local = String::new();
    for c in lower.split('@').next().unwrap_or_default().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() { local.push(c); } else if !local.ends_with('-') { local.push('-'); }
    }
    let local: String = local.trim_matches('-').chars().take(24).collect();
    format!("nexus-{}", if local.is_empty() { "guest" } else { &local })
}

pub fn guests_in(policy: &Value) -> Vec<String> { strs(&policy["groups"][GUEST_GROUP]) }

// ---------------------------------------------------------------- invite codes (pure)
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct InviteMachine { pub name: String, pub host: String, pub user: String, pub icon: String }
#[derive(Clone, Debug, Default)]
pub struct Invite { pub tailnet: String, pub machines: Vec<InviteMachine> }

/// A guest pastes this into Nexus to add the Sparks (tailnet addresses + login users). No secrets inside.
pub fn encode_invite(tailnet: Option<&str>, machines: &[InviteMachine]) -> String {
    let mut data = json!({ "v": 1, "machines": machines });
    if let Some(t) = tailnet { data["tailnet"] = json!(t); }
    format!("{INVITE_PREFIX}{}", B64URL.encode(data.to_string()))
}

/// Accepts the bare code or the whole invitation message pasted, even if a mail client wrapped the code's line.
/// local_tailnet (optional): the tailnet this Nexus is signed in to. A code written for another tailnet is refused, so a
/// guest who is already on some other tailnet (work, family) can't be handed a code that points Nexus at a host there.
pub fn decode_invite(code: &str, local_tailnet: Option<&str>) -> Result<Invite, String> {
    let at = code.find(INVITE_PREFIX).ok_or("Not a Nexus invite code (it starts with nexus-invite:)")?;
    // A wrapped code continues on the next line(s); words after it don't belong to it. Only the right number of pieces
    // decodes to valid JSON, so try 1, 2, 3… pieces.
    let is_b64 = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let rest = &code[at + INVITE_PREFIX.len()..];
    let end = rest.find(|c: char| !is_b64(c) && !c.is_whitespace()).unwrap_or(rest.len());
    let pieces: Vec<&str> = rest[..end].split_whitespace().collect();
    let mut data: Option<Value> = None;
    for n in 1..=pieces.len().min(12) {
        if let Some(v) = B64URL.decode(pieces[..n].concat()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).filter(Value::is_object) { data = Some(v); break; }
    }
    let data = data.ok_or("That invite code is incomplete or damaged. Copy it again from the invitation")?;
    let list = data["machines"].as_array().filter(|_| data["v"] == 1).ok_or("Unsupported invite code")?;
    let tailnet = data["tailnet"].as_str().unwrap_or_default().trim().to_lowercase();
    if let Some(local) = local_tailnet.filter(|l| !tailnet.is_empty() && tailnet != l.trim().to_lowercase()) {
        return Err(format!("This invite is for the tailnet \"{tailnet}\", but this computer is signed in to \"{local}\". Accept the Tailscale invite for that tailnet first"));
    }
    // Only tailnet addresses: an invite can't point Nexus at an arbitrary internet host (which would then receive your
    // SSH login attempts, terminal sessions, team-chat presence and anything you paste or drop into those panes).
    // Hosts start with a word character so they can never be read as an ssh option (-oProxyCommand=…).
    let host_ok = |h: &str| h.chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') && h.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)) && is_tailnet_address(h);
    let machines: Vec<InviteMachine> = list.iter().filter_map(|m| {
        let host = m["host"].as_str().filter(|h| host_ok(h))?;
        Some(InviteMachine {
            name: m["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(host).chars().take(40).collect(),
            host: host.to_string(),
            user: m["user"].as_str().unwrap_or_default().chars().filter(|c| c.is_ascii_alphanumeric() || "_.-".contains(*c)).collect(),
            icon: if m["icon"] == "nvidia" { "nvidia".into() } else { String::new() },
        })
    }).collect();
    if machines.is_empty() { return Err("That invite code has no machines on a Tailscale network in it".into()); }
    Ok(Invite { tailnet, machines })
}

#[derive(Clone, Debug, Default)]
pub struct SparkArg { pub machine_id: String, pub hostname: String, pub ts_host: String }
#[derive(Clone, Debug, Default)]
pub struct Device { pub id: String, pub name: String, pub tags: Vec<String> }
#[derive(Clone, Debug, Default)]
pub struct SparkMatch { pub machine_id: String, pub hostname: String, pub ambiguous: bool, pub device: Option<Device> }

/// Match configured SSH machines to tailnet devices. Prefer the machine's recorded Tailscale name (exact); fall back to
/// the hostname the machine reports only when exactly one device has it; never pick this PC itself. Tagging a device
/// gives guests SSH to it, so a machine that renames itself to match another device must not get that device tagged.
pub fn match_devices(devices: &[Value], machines: &[SparkArg], self_dns_name: &str) -> Vec<SparkMatch> {
    let norm = |s: &str| s.to_lowercase().trim_end_matches('.').to_string();
    let me = norm(self_dns_name);
    let pool: Vec<&Value> = devices.iter().filter(|d| me.is_empty() || norm(d["name"].as_str().unwrap_or_default()) != me).collect();
    machines.iter().map(|m| {
        let mut d = if m.ts_host.is_empty() { None } else { pool.iter().find(|x| norm(x["name"].as_str().unwrap_or_default()) == norm(&m.ts_host) || has(&x["addresses"], &m.ts_host)).copied() };
        let mut ambiguous = false;
        if d.is_none() && !m.hostname.is_empty() {
            let hits: Vec<&&Value> = pool.iter().filter(|x| norm(x["hostname"].as_str().unwrap_or_default()) == norm(&m.hostname)).collect();
            if hits.len() == 1 { d = Some(hits[0]); } else { ambiguous = hits.len() > 1; }
        }
        SparkMatch {
            machine_id: m.machine_id.clone(), hostname: m.hostname.clone(), ambiguous,
            device: d.map(|d| Device { id: d["id"].as_str().or(d["nodeId"].as_str()).unwrap_or_default().to_string(), name: s(&d["name"]), tags: strs(&d["tags"]) }),
        }
    }).collect()
}

/// *.ts.net names and the CGNAT range Tailscale hands out (100.64.0.0/10).
pub fn is_tailnet_address(h: &str) -> bool {
    if h.to_lowercase().ends_with(".ts.net") { return true; }
    let mut it = h.split('.');
    it.next() == Some("100") && it.next().is_some_and(|b| b.len() <= 3 && b.bytes().all(|c| c.is_ascii_digit()) && b.parse::<u16>().is_ok_and(|n| (64..=127).contains(&n))) && it.next().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_policy() -> Value {
        // What a fresh tailnet ships with: everyone reaches everything, SSH to own devices in check mode.
        json!({ "grants": [{ "src": ["*"], "dst": ["*"], "ip": ["*"] }], "ssh": [{ "action": "check", "src": ["autogroup:member"], "dst": ["autogroup:self"], "users": ["autogroup:nonroot", "root"] }] })
    }
    fn opts(guests: &[&str], users: &[&str], per: bool) -> PlanOpts {
        PlanOpts { guests: guests.iter().map(|s| s.to_string()).collect(), ssh_users: users.iter().map(|s| s.to_string()).collect(), per_guest_users: per }
    }
    fn rules(p: &Value, k: &str) -> Vec<Value> { p[k].as_array().cloned().unwrap_or_default() }

    #[test]
    fn plan_locks_guests_to_ssh_and_keeps_owner_connected() {
        let base = default_policy();
        let (policy, changes) = plan_policy(&base, &opts(&["Friend@Example.com"], &["jcee", "jcee-slave"], false));
        assert!(changes.len() >= 5);
        assert_eq!(policy["groups"][GUEST_GROUP], json!(["friend@example.com"]));
        assert_eq!(policy["tagOwners"][SPARK_TAG], json!(["autogroup:admin"]));
        for g in rules(&policy, "grants") { if has(&g["dst"], "*") { assert_eq!(g["src"], json!(["autogroup:admin"])); } }
        assert!(rules(&policy, "grants").iter().any(|g| g["src"][0] == GUEST_GROUP && g["dst"][0] == SPARK_TAG && g["ip"][0] == "tcp:22"));
        assert!(rules(&policy, "grants").iter().any(|g| g["src"][0] == "autogroup:member" && g["dst"][0] == "autogroup:self"));
        let ssh = rules(&policy, "ssh");
        let rule = ssh.iter().find(|r| r["dst"][0] == SPARK_TAG).unwrap();
        assert_eq!(rule["users"], json!(["jcee", "jcee-slave"]));
        assert_eq!(rule["action"], "accept");
        assert!(ssh.iter().any(|r| r["action"] == "check" && r["dst"][0] == "autogroup:self"));
        assert_eq!(base["grants"][0]["src"], json!(["*"]), "input is not mutated");
    }
    #[test]
    fn plan_is_idempotent() {
        let first = plan_policy(&default_policy(), &opts(&["a@x.com"], &["jcee"], false)).0;
        let (again, changes) = plan_policy(&first, &opts(&["a@x.com"], &["jcee"], false));
        assert!(changes.is_empty(), "{changes:?}");
        assert_eq!(again, first);
        let (more, changes) = plan_policy(&first, &opts(&["a@x.com", "b@x.com"], &["jcee"], false));
        assert_eq!(changes.len(), 1);
        assert_eq!(guests_in(&more), ["a@x.com", "b@x.com"]);
    }
    #[test]
    fn plan_narrows_legacy_acls() {
        let legacy = json!({ "acls": [{ "action": "accept", "src": ["*"], "dst": ["*:*"] }, { "action": "accept", "src": ["group:ops"], "dst": ["tag:db:5432"] }] });
        let (policy, _) = plan_policy(&legacy, &opts(&[], &["jcee"], false));
        assert_eq!(policy["acls"][0]["src"], json!(["autogroup:admin"]));
        assert_eq!(policy["acls"][1]["src"], json!(["group:ops"]));
    }
    #[test]
    fn plan_per_guest_mode() {
        assert_eq!(guest_user("Alice.Smith+gpu@example.com"), "nexus-alice-smith-gpu");
        let (first, _) = plan_policy(&default_policy(), &opts(&["alice@x.com", "bob@x.com"], &["jcee"], true));
        let ssh = rules(&first, "ssh");
        let shared = ssh.iter().find(|r| r["dst"][0] == SPARK_TAG && r["src"] == json!(["autogroup:admin"])).unwrap();
        assert_eq!(shared["users"], json!(["jcee"]));
        assert!(!ssh.iter().any(|r| has(&r["src"], GUEST_GROUP)), "guests do not get the shared account");
        assert_eq!(ssh.iter().find(|r| r["src"][0] == "alice@x.com").unwrap()["users"], json!(["nexus-alice"]));
        assert!(plan_policy(&first, &opts(&["alice@x.com", "bob@x.com"], &["jcee"], true)).1.is_empty());
        let (less, changes) = plan_policy(&first, &opts(&["alice@x.com"], &["jcee"], true));
        assert!(!rules(&less, "ssh").iter().any(|r| r["src"][0] == "bob@x.com"));
        assert!(changes.iter().any(|c| c.contains("remove bob@x.com")));
        let shared_mode = plan_policy(&default_policy(), &opts(&["alice@x.com"], &["jcee"], false)).0;
        let (moved, changes) = plan_policy(&shared_mode, &opts(&["alice@x.com"], &["jcee"], true));
        assert!(!rules(&moved, "ssh").iter().any(|r| has(&r["src"], GUEST_GROUP) && r["dst"][0] == SPARK_TAG));
        assert!(changes.iter().any(|c| c.contains("no longer share")));
    }
    fn im(name: &str, host: &str, user: &str) -> InviteMachine { InviteMachine { name: name.into(), host: host.into(), user: user.into(), icon: String::new() } }
    #[test]
    fn invite_codes_round_trip_and_reject_junk() {
        let code = encode_invite(Some("x.ts.net"), &[InviteMachine { icon: "nvidia".into(), ..im("Spark 1", "spark1-aresj.tail1.ts.net", "jcee") }]);
        assert!(code.starts_with("nexus-invite:"));
        let back = decode_invite(&code, None).unwrap();
        assert_eq!((back.machines[0].host.as_str(), back.machines[0].user.as_str(), back.machines[0].icon.as_str()), ("spark1-aresj.tail1.ts.net", "jcee", "nvidia"));
        assert!(decode_invite("hello", None).is_err());
        let evil = encode_invite(None, &[im("x", "a; rm -rf ~", "u$(id)"), im("", "ok.ts.net", "jcee")]);
        let d = decode_invite(&evil, None).unwrap();
        assert_eq!(d.machines.iter().map(|m| m.host.as_str()).collect::<Vec<_>>(), ["ok.ts.net"]);
        assert_eq!(d.machines[0].name, "ok.ts.net");
        assert!(decode_invite(&code, Some("other.ts.net")).unwrap_err().contains("is for the tailnet"));
        assert!(decode_invite(&code, Some("X.ts.net")).is_ok());
    }
    #[test]
    fn invite_codes_in_wrapped_messages_and_damaged() {
        let code = encode_invite(Some("x.ts.net"), &[im("Spark 1", "spark1.tail1.ts.net", "jcee")]);
        let wrapped = format!("{}\r\n   {}", &code[..30], &code[30..]);
        let message = format!("3. In Nexus click Share, paste this invite code:\n   {wrapped}\n\nYou'll log in as Spark 1: jcee.");
        assert_eq!(decode_invite(&message, None).unwrap().machines[0].host, "spark1.tail1.ts.net");
        assert!(decode_invite("nexus-invite:garbage", None).unwrap_err().contains("incomplete or damaged"));
        assert!(decode_invite(&code[..40], None).unwrap_err().contains("incomplete or damaged"));
        assert!(decode_invite(&encode_invite(None, &[]), None).unwrap_err().contains("no machines"));
    }
    #[test]
    fn tailnet_addresses() {
        assert!(is_tailnet_address("100.101.2.3"));
        assert!(is_tailnet_address("spark1.tail1234.ts.net"));
        assert!(!is_tailnet_address("192.168.8.236"));
        assert!(!is_tailnet_address("100.200.1.1"));
        assert!(!is_tailnet_address("100.63.1.1"));
        let code = encode_invite(None, &[im("evil", "attacker.example.com", "x"), im("ip", "203.0.113.5", "x"), im("ok", "100.101.2.3", "jcee")]);
        assert_eq!(decode_invite(&code, None).unwrap().machines.len(), 1);
        assert!(decode_invite(&encode_invite(None, &[im("", "lan-box.local", "x")]), None).unwrap_err().contains("Tailscale"));
    }
    #[test]
    fn match_devices_name_first_never_this_pc() {
        let devices = vec![
            json!({ "id": "pc", "name": "my-pc.tail1.ts.net", "hostname": "spark1", "tags": [] }),
            json!({ "id": "s1", "name": "spark1.tail1.ts.net", "hostname": "spark1", "tags": [] }),
            json!({ "id": "a", "name": "dup-a.tail1.ts.net", "hostname": "dup", "tags": [] }),
            json!({ "id": "b", "name": "dup-b.tail1.ts.net", "hostname": "dup", "tags": [] }),
        ];
        let a = |id: &str, h: &str, t: &str| SparkArg { machine_id: id.into(), hostname: h.into(), ts_host: t.into() };
        let r = match_devices(&devices, &[a("m1", "whatever", "spark1.tail1.ts.net"), a("m2", "spark1", ""), a("m3", "dup", ""), a("m4", "my-pc", "my-pc.tail1.ts.net")], "my-pc.tail1.ts.net.");
        assert_eq!(r[0].device.as_ref().unwrap().id, "s1");
        assert_eq!(r[1].device.as_ref().unwrap().id, "s1", "this PC (same hostname) is excluded, leaving one match");
        assert!(r[2].device.is_none() && r[2].ambiguous);
        assert!(r[3].device.is_none(), "never tag this PC");
    }
    // Credentials are assembled at run time so no string in this file looks like a real Tailscale key to secret scanners.
    fn k(parts: &[&str]) -> String { let mut v = vec!["tskey"]; v.extend(parts); v.join("-") }
    #[test]
    fn credentials_are_told_apart() {
        let (api, client) = (k(&["api", "kFAKE0CNTRL", "xyz"]), k(&["client", "kFAKE0CNTRL", "secretPart"]));
        assert_eq!(parse_credential(&format!(" {api} ")), Some(Credential::Api { token: api.clone() }));
        assert_eq!(parse_credential(&client), Some(Credential::OAuth { client_id: None, secret: client.clone() }));
        assert_eq!(parse_credential(&format!("kFAKE0CNTRL:{client}")), Some(Credential::OAuth { client_id: Some("kFAKE0CNTRL".into()), secret: client }));
        for bad in [String::new(), k(&["auth", "abc"]), format!("Bearer {api}"), format!("{api} y")] { assert_eq!(parse_credential(&bad), None, "{bad}"); }
    }
    #[test]
    fn oauth_clients_exchange_the_secret_once_per_hour() {
        let (fresh, mine, secret) = (k(&["api", "fresh"]), k(&["api", "mine"]), k(&["client", "secret"]));
        let calls: Arc<Mutex<Vec<(String, String, String)>>> = Arc::default();
        let (c2, f2) = (calls.clone(), fresh.clone());
        let http: Http = Arc::new(move |r: HttpReq| {
            let auth = r.headers.iter().find(|(k, _)| k == "Authorization").map(|(_, v)| v.clone()).unwrap_or_default();
            c2.lock().unwrap().push((r.url.clone(), auth, r.body.unwrap_or_default()));
            let body = if r.url.ends_with("/oauth/token") { json!({ "access_token": f2, "expires_in": 3600 }) } else { json!({ "devices": [{ "id": 1 }] }) };
            Ok(HttpRes { status: 200, body: body.to_string(), etag: None })
        });
        let api = TailscaleApi::new(parse_credential(&format!("kID:{secret}")).unwrap(), http.clone());
        assert_eq!(api.devices().unwrap().len(), 1);
        api.devices().unwrap();
        let seen = calls.lock().unwrap().clone();
        assert_eq!(seen.iter().filter(|c| c.0.ends_with("/oauth/token")).count(), 1, "one exchange serves both calls");
        assert!(seen[0].2.contains(&format!("client_secret={secret}")) && seen[0].2.contains("client_id=kID"));
        assert_eq!(seen[1].1, format!("Bearer {fresh}"));
        TailscaleApi::new(parse_credential(&mine).unwrap(), http).devices().unwrap();
        assert_eq!(calls.lock().unwrap().last().unwrap().1, format!("Bearer {mine}"), "API tokens are used as-is");
    }
    #[test]
    fn local_status_json() {
        let st = parse_local_status(&json!({ "BackendState": "Running", "CurrentTailnet": { "Name": "me.ts.net" }, "Self": { "HostName": "pc", "DNSName": "pc.me.ts.net.", "UserID": 7, "Online": true, "TailscaleIPs": ["100.64.0.1"] },
            "User": { "7": { "LoginName": "me@x.com", "DisplayName": "Me" } }, "Peer": { "k": { "HostName": "spark1", "DNSName": "spark1.me.ts.net.", "Online": false } } }));
        assert_eq!(st.self_peer.unwrap().dns_name, "pc.me.ts.net");
        assert_eq!(st.user.unwrap().name, "Me");
        assert_eq!(st.peers[0].host_name, "spark1");
    }
}
