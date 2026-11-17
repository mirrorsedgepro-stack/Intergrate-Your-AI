//! Pure helpers shared by the UI: palette matching, chat mentions, claims, small formatters.
use crate::collab::Presence;
use std::collections::HashSet;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Palette matching. Each space-separated word must appear in order as a subsequence of the text, but the words
/// themselves may come in any order: "spark 1 shell" finds "Shell in folder… on Spark 1". Returns None for no match,
/// else (score, hits) where hits are the matched character positions (for highlighting).
pub fn fuzzy(q: &str, text: &str) -> Option<(f64, HashSet<usize>)> {
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut hits = HashSet::new();
    let mut score = 0.0;
    for w in q.to_lowercase().split_whitespace() {
        let w: Vec<char> = w.chars().collect();
        let (s, h) = best_word(&w, &t)?;
        score += s;
        hits.extend(h);
    }
    Some((score - t.len() as f64 * 0.02, hits))
}

fn find_from(t: &[char], ch: char, from: usize) -> Option<usize> {
    t.iter().skip(from).position(|&c| c == ch).map(|i| i + from)
}

/// Best placement of one word: try each occurrence of its first letter and keep the highest-scoring subsequence.
fn best_word(w: &[char], t: &[char]) -> Option<(f64, Vec<usize>)> {
    let mut best: Option<(f64, Vec<usize>)> = None;
    let mut start = find_from(t, *w.first()?, 0);
    while let Some(st) = start {
        let (mut ti, mut score, mut last) = (st, 0.0, None::<usize>);
        let mut hits = Vec::new();
        let mut ok = true;
        for &ch in w {
            let Some(j) = find_from(t, ch, ti) else { ok = false; break };
            score += if last.is_some_and(|l| j == l + 1) { 6.0 } else { 1.0 };
            if j == 0 || " -·@(/".contains(t[j - 1]) { score += 4.0; }
            hits.push(j);
            last = Some(j);
            ti = j + 1;
        }
        if !ok { break; } // later starts can't match either
        if best.as_ref().is_none_or(|b| score > b.0) { best = Some((score, hits)); }
        start = find_from(t, w[0], st + 1);
    }
    best
}

const fn is_word(c: char) -> bool { c.is_ascii_alphanumeric() || c == '_' }

/// Does a chat message @mention this person? Matches @first-name (case-insensitive), whole word only.
pub fn mentions(text: &str, name: &str) -> bool {
    let Some(first) = name.split_whitespace().next() else { return false };
    let first: Vec<char> = first.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    for i in 0..t.len() {
        if t[i] != '@' || (i > 0 && (is_word(t[i - 1]) || t[i - 1] == '@')) { continue; }
        let end = i + 1 + first.len();
        if end <= t.len() && t[i + 1..end] == first[..] && t.get(end).is_none_or(|&c| !is_word(c) && c != '-') { return true; }
    }
    false
}

/// Windows shells under ConPTY title themselves with their executable's path ("C:\...\pwsh.exe"): not a useful title.
pub fn is_exe_title(title: &str) -> bool {
    let t = title.to_lowercase();
    let t = t.strip_prefix("administrator:").map(str::trim_start).unwrap_or(&t);
    if !t.ends_with(".exe") { return false; }
    let b = t.as_bytes();
    let drive = b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
    drive || t.starts_with("\\\\") || t.chars().all(|c| is_word(c) || c == '.' || c == '-')
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClaimReq { pub host: String, pub until: i64, pub note: String }

/// Parse "/claim <host> <duration> [note]" where duration is like 2h, 45m, 1h30m, or a clock time 15:00.
pub fn parse_claim(text: &str, now: i64) -> Option<ClaimReq> {
    let rest = text.strip_prefix("/claim")?;
    if !rest.starts_with(char::is_whitespace) { return None; }
    let rest = rest.trim_start();
    let (host, rest) = rest.split_once(char::is_whitespace)?;
    let rest = rest.trim_start();
    let (dur, note) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let until = if let Some((h, m)) = dur.split_once(':') {
        use chrono::{Duration, Local, TimeZone};
        if h.is_empty() || h.len() > 2 || m.len() != 2 { return None; }
        let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
        let base = Local.timestamp_millis_opt(now).single()?;
        let mut t = Local.from_local_datetime(&base.date_naive().and_hms_opt(h, m, 0)?).earliest()?;
        if t.timestamp_millis() <= now { t += Duration::days(1); }
        t.timestamp_millis()
    } else {
        let (h, m) = match dur.split_once('h') { Some((h, m)) => (Some(h), m), None => (None, dur) };
        let m = if m.is_empty() { None } else { Some(m.strip_suffix('m')?) };
        if h.is_none() && m.is_none() { return None; }
        let num = |s: Option<&str>| -> Option<i64> { match s { None => Some(0), Some(s) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => s.parse().ok(), _ => None } };
        now + (num(h)? * 60 + num(m)?) * 60_000
    };
    Some(ClaimReq { host: host.chars().take(64).collect(), until, note: note.trim().chars().take(120).collect() })
}

#[derive(Clone, Debug)]
pub struct ClaimHit { pub person: String, pub until: i64, pub note: String }

/// Who currently claims `host` (presence entries other than `client`, claims not yet expired).
pub fn claims_on(presence: &[Presence], host: &str, client: Option<&str>, now: i64) -> Vec<ClaimHit> {
    let mut out = Vec::new();
    for p in presence {
        if client.is_some_and(|c| c == p.client) { continue; }
        for c in &p.claims {
            if c.host == host && c.until > now { out.push(ClaimHit { person: p.name.clone(), until: c.until, note: c.note.clone() }); }
        }
    }
    out
}

pub fn base_name(p: &str) -> &str {
    p.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).unwrap_or(p)
}
pub fn short_path(p: &str) -> String {
    let n = p.chars().count();
    if n > 34 { format!("…{}", p.chars().skip(n - 33).collect::<String>()) } else { p.to_string() }
}
pub fn fmt_time(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    Local.timestamp_millis_opt(ts).single().map(|t| t.format("%H:%M").to_string()).unwrap_or_default()
}
pub fn gb(b: f64) -> String {
    let g = b / 1024f64.powi(3);
    if g >= 100.0 { format!("{g:.0}") } else { format!("{g:.1}") }
}
/// Bytes/second -> "12.3 MB/s" (short, for a meter value).
pub fn bps(v: Option<f64>) -> String {
    match v {
        None => "-".into(),
        Some(v) if v >= 1024.0 * 1024.0 => { let m = v / 1024.0 / 1024.0; if m >= 10.0 { format!("{m:.0} MB/s") } else { format!("{m:.1} MB/s") } }
        Some(v) if v >= 1024.0 => format!("{:.0} kB/s", v / 1024.0),
        Some(v) => format!("{v:.0} B/s"),
    }
}
/// Truncate to at most `n` characters.
pub fn clip(s: &str, n: usize) -> String { s.chars().take(n).collect() }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_words_any_order_letters_in_order() {
        assert!(fuzzy("spark 1 shell", "Shell in folder… on Spark 1").is_some());
        assert!(fuzzy("clde", "New Claude on Linux box").is_some());
        assert!(fuzzy("edualc", "Claude").is_none());
        assert!(fuzzy("zzz", "Claude").is_none());
        assert_eq!(fuzzy("", "anything").unwrap().1.len(), 0);
    }
    #[test]
    fn fuzzy_ranks_tight_and_word_start_matches_higher() {
        let a = fuzzy("claude", "New Claude").unwrap().0;
        let b = fuzzy("claude", "Close all und everything").map(|x| x.0).unwrap_or(f64::MIN);
        assert!(a > b);
        assert!(fuzzy("sh", "Shell").unwrap().0 > fuzzy("sh", "Flash").unwrap().0);
    }
    #[test]
    fn mentions_first_name_whole_word() {
        assert!(mentions("hey @sam can you look", "Sam Jones"));
        assert!(mentions("@SAM!", "sam"));
        assert!(!mentions("hey @samantha", "Sam"));
        assert!(!mentions("mail sam@example.com", "example"));
        assert!(!mentions("no mention", "Sam"));
        assert!(!mentions("@", ""));
    }
    #[test]
    fn exe_titles() {
        assert!(is_exe_title(r"C:\Program Files\PowerShell\7\pwsh.exe"));
        assert!(is_exe_title(r"Administrator: C:\Windows\System32\cmd.exe"));
        assert!(is_exe_title("powershell.exe"));
        assert!(!is_exe_title("claude: fixing tests"));
        assert!(!is_exe_title("~/code/api"));
    }
    #[test]
    fn claim_parsing_and_expiry() {
        let now = 1_700_000_000_000;
        let c = parse_claim("/claim gpu-box 2h fine-tuning run", now).unwrap();
        assert_eq!((c.host.as_str(), c.until, c.note.as_str()), ("gpu-box", now + 7_200_000, "fine-tuning run"));
        assert_eq!(parse_claim("/claim box 1h30m", now).unwrap().until, now + 5_400_000);
        assert_eq!(parse_claim("/claim box 45m", now).unwrap().until, now + 2_700_000);
        let clock = parse_claim("/claim box 15:00", now).unwrap().until;
        assert!(clock > now && clock <= now + 86_400_000);
        assert!(parse_claim("/claim box soon", now).is_none());
        assert!(parse_claim("/claim box", now).is_none());
        assert!(parse_claim("/claimbox 2h", now).is_none());
        let p = |client: &str, until| Presence { client: client.into(), name: client.to_uppercase(), claims: vec![crate::collab::Claim { host: "h".into(), until, note: String::new() }], ..Default::default() };
        let pres = vec![p("a", now + 1000), p("b", now - 1), p("me", now + 1000)];
        let hits = claims_on(&pres, "h", Some("me"), now);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].person, "A");
    }
}
