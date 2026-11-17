//! Minimal OpenSSH client config + known_hosts support, so Nexus resolves hosts the way `ssh` does.
use crate::platform::{expand_home, home};
use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Digest;
use std::path::{Path, PathBuf};

pub fn ssh_dir() -> PathBuf { home().join(".ssh") }
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

fn unquote(s: &str) -> &str { s.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(s) }
/// Split on whitespace, keeping "quoted words" together (quotes removed).
fn words(v: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
    for c in v.chars() {
        if c == '"' { quoted = !quoted; any = true; }
        else if c.is_whitespace() && !quoted { if any || !cur.is_empty() { out.push(std::mem::take(&mut cur)); any = false; } }
        else { cur.push(c); }
    }
    if any || !cur.is_empty() { out.push(cur); }
    out.into_iter().filter(|w| !w.is_empty()).collect()
}

// ---------------------------------------------------------------- ssh_config
/// Relative Include paths resolve against the top-level config's directory (~/.ssh), as OpenSSH does.
pub fn read_ssh_config(file: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    read_into(file, 0, &mut out, file.parent().unwrap_or(Path::new(".")));
    out
}
fn read_into(file: &Path, depth: usize, out: &mut Vec<(String, String)>, base: &Path) {
    if depth > 8 { return; }
    let Ok(text) = std::fs::read_to_string(file) else { return };
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        // "Key value" or "Key=value" (with optional blanks around the =).
        let Some(end) = line.find(|c: char| c.is_whitespace() || c == '=') else { continue };
        let (key, rest) = (line[..end].to_lowercase(), line[end..].trim_start());
        let value = rest.strip_prefix('=').unwrap_or(rest).trim();
        if key.is_empty() { continue; }
        if key == "include" {
            for inc in words(value) {
                let mut p = expand_home(&inc);
                if !p.is_absolute() { p = base.join(p); }
                read_into(&p, depth + 1, out, base);
            }
        } else { out.push((key, value.to_string())); }
    }
}

/// Case-insensitive glob with `*` and `?`.
pub fn glob_match(pattern: &str, s: &str) -> bool {
    fn go(p: &[char], s: &[char]) -> bool {
        match p.split_first() {
            None => s.is_empty(),
            Some((&'*', rest)) => (0..=s.len()).any(|i| go(rest, &s[i..])),
            Some((&c, rest)) => s.split_first().is_some_and(|(&x, srest)| (c == '?' || c == x) && go(rest, srest)),
        }
    }
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let s: Vec<char> = s.to_lowercase().chars().collect();
    go(&p, &s)
}

fn pats_match<S: AsRef<str>>(pats: &[S], name: &str) -> bool {
    pats.iter().any(|p| !p.as_ref().starts_with('!') && glob_match(p.as_ref(), name))
        && !pats.iter().any(|p| p.as_ref().strip_prefix('!').is_some_and(|n| glob_match(n, name)))
}

/// Does a "Match" line apply to this alias? Supports: all, host <patterns>, originalhost <patterns>, and "final"
/// (treated like all). Criteria Nexus can't evaluate (user, localuser, exec, canonical, tagged) make the block inactive,
/// which is the safe reading: options in it are simply not applied.
pub fn match_applies(criteria: &str, alias: &str, hostname: Option<&str>) -> bool {
    let w = words(criteria);
    if w.is_empty() { return false; }
    let (mut i, mut ok) = (0, true);
    while i < w.len() {
        let mut k = w[i].to_lowercase();
        let neg = k.starts_with('!');
        if neg { k.remove(0); }
        let mut hit = match k.as_str() {
            "all" | "final" => { i += 1; true }
            "canonical" => { i += 1; false }
            "host" | "originalhost" => {
                let pats: Vec<&str> = w.get(i + 1).map(|s| s.split(',').collect()).unwrap_or_default();
                i += 2;
                pats_match(&pats, if k == "host" { hostname.unwrap_or(alias) } else { alias }) || pats_match(&pats, alias)
            }
            _ => return false, // user / localuser / exec / tagged: not evaluated
        };
        if neg { hit = !hit; }
        ok = ok && hit;
    }
    ok
}

#[derive(Clone, Debug, PartialEq)]
pub struct Jump { pub user: Option<String>, pub host: String, pub port: Option<u16> }

/// "user@host:port" (or "host", "[::1]:22") from a ProxyJump value.
pub fn parse_jump(spec: &str) -> Option<Jump> {
    let spec = spec.trim();
    let (user, rest) = match spec.split_once('@') { Some((u, r)) if !u.is_empty() => (Some(u.to_string()), r), Some(_) => return None, None => (None, spec) };
    let (host, port) = if let Some(r) = rest.strip_prefix('[') {
        let (h, tail) = r.split_once(']')?;
        (h, match tail.strip_prefix(':') { Some(p) => Some(p), None if tail.is_empty() => None, None => return None })
    } else {
        match rest.split_once(':') { Some((h, p)) => (h, Some(p)), None => (rest, None) }
    };
    if host.is_empty() || host.contains(':') && !rest.starts_with('[') { return None; }
    let port = match port { Some(p) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => Some(p.parse().ok()?), Some(_) => return None, None => None };
    Some(Jump { user, host: host.to_string(), port })
}

#[derive(Clone, Debug, Default)]
pub struct HostConfig {
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_files: Vec<PathBuf>,
    pub known_hosts_files: Vec<PathBuf>,
    /// The ProxyJump chain (first hop first), or empty for a direct connection.
    pub jumps: Vec<Jump>,
}

/// Resolve options for a host alias. First value wins, IdentityFile/UserKnownHostsFile accumulate.
pub fn ssh_config_for(alias: &str) -> HostConfig { ssh_config_for_in(alias, &ssh_dir().join("config")) }
pub fn ssh_config_for_in(alias: &str, config_file: &Path) -> HostConfig {
    let mut res = HostConfig::default();
    let (mut active, mut proxy_jump) = (true, None::<String>);
    for (key, value) in read_ssh_config(config_file) {
        match key.as_str() {
            "host" => { active = pats_match(&words(&value), alias); continue; }
            "match" => { active = match_applies(&value, alias, res.hostname.as_deref()); continue; }
            _ if !active => continue,
            "identityfile" => res.identity_files.push(expand_home(&value)),
            "userknownhostsfile" => res.known_hosts_files.extend(words(&value).iter().map(|w| expand_home(w))),
            "hostname" if res.hostname.is_none() => res.hostname = Some(unquote(&value).to_string()),
            "user" if res.user.is_none() => res.user = Some(unquote(&value).to_string()),
            "port" if res.port.is_none() => res.port = value.parse().ok(),
            "proxyjump" if proxy_jump.is_none() => proxy_jump = Some(value),
            _ => {}
        }
    }
    if let Some(pj) = proxy_jump.filter(|p| !p.eq_ignore_ascii_case("none")) { res.jumps = pj.split(',').filter_map(parse_jump).collect(); }
    res
}

// ---------------------------------------------------------------- known_hosts
/// The host key arrives as an SSH wire-format blob: string keytype, then key data.
pub fn blob_type(blob: &[u8]) -> String {
    let len = blob.get(..4).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
    blob.get(4..4 + len).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default()
}

fn host_field_matches(field: &str, names: &[String]) -> bool {
    let mut hit = false;
    for pat in field.split(',') {
        if let Some(h) = pat.strip_prefix("|1|") {
            let Some((salt, hash)) = h.split_once('|') else { continue };
            let Ok(salt) = B64.decode(salt) else { continue };
            for n in names {
                let Ok(mut mac) = Hmac::<sha1::Sha1>::new_from_slice(&salt) else { continue };
                mac.update(n.as_bytes());
                if B64.encode(mac.finalize().into_bytes()) == hash { hit = true; }
            }
        } else if let Some(neg) = pat.strip_prefix('!') {
            if names.iter().any(|n| glob_match(neg, n)) { return false; }
        } else if names.iter().any(|n| glob_match(pat, n)) { hit = true; }
    }
    hit
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Verdict { Match, /** Same key type, different key: possible MITM. */ Mismatch, Revoked, Unknown }

pub fn check_known_hosts(files: &[PathBuf], host: &str, port: u16, blob: &[u8]) -> Verdict {
    let names = vec![if port == 22 { host.to_string() } else { format!("[{host}]:{port}") }];
    let ktype = blob_type(blob);
    let b64 = B64.encode(blob);
    let (mut mismatch, mut matched) = (false, false);
    for file in files {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            let mut parts: Vec<&str> = line.split_whitespace().collect();
            let marker = if parts[0].starts_with('@') { Some(parts.remove(0)) } else { None };
            if marker == Some("@cert-authority") || parts.len() < 3 { continue; }
            let (hosts, kt, key) = (parts[0], parts[1], parts[2]);
            if marker == Some("@revoked") { if key == b64 { return Verdict::Revoked; } continue; }
            if !host_field_matches(hosts, &names) { continue; }
            if key == b64 { matched = true; } else if kt == ktype { mismatch = true; }
        }
    }
    // A revoked key always loses (checked above, wherever it appears); an exact match beats a stale entry.
    if matched { Verdict::Match } else if mismatch { Verdict::Mismatch } else { Verdict::Unknown }
}

pub fn fingerprint(blob: &[u8]) -> String {
    format!("SHA256:{}", B64.encode(sha2::Sha256::digest(blob)).trim_end_matches('='))
}

#[cfg(test)]
pub(crate) fn test_blob(ktype: &str, seed: u8) -> Vec<u8> {
    let key: Vec<u8> = (0..32u8).map(|i| i.wrapping_mul(7).wrapping_add(seed)).collect();
    let mut b = (ktype.len() as u32).to_be_bytes().to_vec();
    b.extend(ktype.as_bytes());
    b.extend((key.len() as u32).to_be_bytes());
    b.extend(key);
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nexus-ssh-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p
    }
    fn names(v: &[PathBuf]) -> Vec<String> { v.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect() }

    #[test]
    fn include_first_value_wins_identity_files_accumulate() {
        let d = tmp("inc");
        let vendor = write(&d, "vendor/conf", "Host 10.0.0.5\n  User vendor\n  IdentityFile ~/vendor.key\n  UserKnownHostsFile ~/vendor_known\n");
        let cfg = write(&d, "config", &format!("Include \"{}\"\nHost 10.0.0.5\n  User override-ignored\n  IdentityFile ~/.ssh/second\nHost box\n  HostName = 10.0.0.9\n  Port 2222\nHost *\n  User fallback", vendor.display()));
        let a = ssh_config_for_in("10.0.0.5", &cfg);
        assert_eq!(a.user.as_deref(), Some("vendor"));
        assert_eq!(names(&a.identity_files), ["vendor.key", "second"]);
        assert_eq!(names(&a.known_hosts_files), ["vendor_known"]);
        let b = ssh_config_for_in("box", &cfg);
        assert_eq!((b.hostname.as_deref(), b.port, b.user.as_deref()), (Some("10.0.0.9"), Some(2222), Some("fallback")));
        assert!(ssh_config_for_in("other", &cfg).identity_files.is_empty());
    }
    #[test]
    fn negated_patterns_exclude_a_host() {
        let cfg = write(&tmp("neg"), "config", "Host * !secret\n  User everyone\n");
        assert_eq!(ssh_config_for_in("a", &cfg).user.as_deref(), Some("everyone"));
        assert_eq!(ssh_config_for_in("secret", &cfg).user, None);
    }
    #[test]
    fn known_hosts_plain_hashed_port_mismatch_revoked() {
        let (good, other, rsa) = (test_blob("ssh-ed25519", 1), test_blob("ssh-ed25519", 2), test_blob("ssh-rsa", 3));
        let salt = [9u8; 20];
        let mut mac = Hmac::<sha1::Sha1>::new_from_slice(&salt).unwrap();
        mac.update(b"10.1.1.1");
        let hashed = format!("|1|{}|{}", B64.encode(salt), B64.encode(mac.finalize().into_bytes()));
        let (g, o, r) = (B64.encode(&good), B64.encode(&other), B64.encode(&rsa));
        let kh = write(&tmp("kh"), "known_hosts", &format!("192.168.8.236,spark1 ssh-ed25519 {g}\n{hashed} ssh-ed25519 {g}\n[10.2.2.2]:2222 ssh-ed25519 {g}\n10.3.3.3 ssh-rsa {r}\n@revoked * ssh-ed25519 {o}\nspark1 ssh-ed25519 {o}"));
        let f = [kh];
        assert_eq!(blob_type(&good), "ssh-ed25519");
        assert_eq!(check_known_hosts(&f, "192.168.8.236", 22, &good), Verdict::Match);
        assert_eq!(check_known_hosts(&f, "10.1.1.1", 22, &good), Verdict::Match);
        assert_eq!(check_known_hosts(&f, "10.2.2.2", 2222, &good), Verdict::Match);
        assert_eq!(check_known_hosts(&f, "10.2.2.2", 22, &good), Verdict::Unknown);
        assert_eq!(check_known_hosts(&f, "10.3.3.3", 22, &good), Verdict::Unknown); // different key type: not a conflict
        assert_eq!(check_known_hosts(&f, "192.168.8.236", 22, &test_blob("ssh-ed25519", 4)), Verdict::Mismatch);
        assert_eq!(check_known_hosts(&f, "192.168.8.236", 22, &other), Verdict::Revoked);
        assert_eq!(check_known_hosts(&[PathBuf::from("/nonexistent/x")], "x", 22, &good), Verdict::Unknown);
        assert!(fingerprint(&good).starts_with("SHA256:") && !fingerprint(&good).ends_with('='));
    }
    #[test]
    fn match_blocks_and_proxy_jump() {
        let cfg = write(&tmp("match"), "config", "Host gpu*\n  ProxyJump ops@bastion.example.com:2222,inner\nMatch host gpu-2\n  Port 2200\nMatch user someone\n  Port 9999\nMatch all\n  User everyone\nHost direct\n  ProxyJump none");
        let a = ssh_config_for_in("gpu-2", &cfg);
        assert_eq!((a.port, a.user.as_deref()), (Some(2200), Some("everyone")));
        assert_eq!(a.jumps, vec![Jump { user: Some("ops".into()), host: "bastion.example.com".into(), port: Some(2222) }, Jump { user: None, host: "inner".into(), port: None }]);
        assert!(ssh_config_for_in("direct", &cfg).jumps.is_empty());
        assert_eq!(ssh_config_for_in("gpu-1", &cfg).port, None);
        assert!(!match_applies("host gpu-*,!gpu-9", "gpu-9", None));
        assert!(match_applies("!host other", "gpu-9", None));
        assert!(!match_applies("exec \"true\"", "gpu-9", None));
        assert_eq!(parse_jump("[fd00::1]:22"), Some(Jump { user: None, host: "fd00::1".into(), port: Some(22) }));
        assert_eq!(parse_jump("-oProxyCommand=x@"), None);
    }
}
