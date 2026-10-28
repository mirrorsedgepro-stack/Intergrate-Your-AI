// Pure helpers shared by the renderer (window.NexusUtil) and the unit tests (require).
(function (root, factory) {
  if (typeof module === 'object' && module.exports) module.exports = factory();
  else root.NexusUtil = factory();
})(typeof self !== 'undefined' ? self : this, () => {
  'use strict';

  // Palette matching. Each space-separated word must appear in order as a subsequence of the text, but the words
  // themselves may come in any order: "spark 1 shell" finds "Shell in folder… on Spark 1". Returns null for no match,
  // else { score, hits } where hits are the matched character positions (for highlighting).
  function fuzzy(q, text) {
    const t = String(text).toLowerCase();
    const words = String(q).toLowerCase().split(/\s+/).filter(Boolean);
    const hits = new Set();
    let score = 0;
    for (const w of words) {
      const m = bestWord(w, t);
      if (!m) return null;
      score += m.score;
      m.hits.forEach((h) => hits.add(h));
    }
    return { score: score - t.length * 0.02, hits };
  }

  // Best placement of one word: try each occurrence of its first letter and keep the highest-scoring subsequence.
  function bestWord(w, t) {
    let best = null;
    for (let start = t.indexOf(w[0]); start >= 0; start = t.indexOf(w[0], start + 1)) {
      let ti = start, score = 0, last = -2;
      const hits = [];
      let ok = true;
      for (const ch of w) {
        const j = t.indexOf(ch, ti);
        if (j < 0) { ok = false; break; }
        score += j === last + 1 ? 6 : 1;
        if (j === 0 || ' -·@(/'.includes(t[j - 1])) score += 4;
        hits.push(j);
        last = j;
        ti = j + 1;
      }
      if (!ok) break; // later starts can't match either
      if (!best || score > best.score) best = { score, hits };
    }
    return best;
  }

  // Does a chat message @mention this person? Matches @first-name (case-insensitive), whole word only.
  function mentions(text, name) {
    const first = String(name || '').trim().split(/\s+/)[0].toLowerCase();
    if (!first) return false;
    const re = new RegExp(`(^|[^\\w@])@${first.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}(?![\\w-])`, 'i');
    return re.test(String(text || ''));
  }

  // Windows shells under ConPTY title themselves with their executable's path ("C:\...\pwsh.exe"): not a useful title.
  function isExeTitle(title) {
    return /^(administrator:\s*)?([a-z]:\\|\\\\).*\.exe$/i.test(title) || /^[\w.-]+\.exe$/i.test(title);
  }

  // Parse "/claim <host> <duration> [note]" where duration is like 2h, 45m, 1h30m, or a clock time 15:00.
  function parseClaim(text, now = Date.now()) {
    const m = /^\/claim\s+(\S+)\s+(\S+)\s*(.*)$/s.exec(String(text || ''));
    if (!m) return null;
    const [, host, dur, note] = m;
    let until = null;
    let d;
    if ((d = /^(\d{1,2}):(\d{2})$/.exec(dur))) {
      const t = new Date(now); t.setHours(Number(d[1]), Number(d[2]), 0, 0);
      if (t.getTime() <= now) t.setDate(t.getDate() + 1);
      until = t.getTime();
    } else if ((d = /^(?:(\d+)h)?(?:(\d+)m)?$/.exec(dur)) && (d[1] || d[2])) {
      until = now + (Number(d[1] || 0) * 60 + Number(d[2] || 0)) * 60000;
    }
    if (!until) return null;
    return { host: host.slice(0, 64), until, note: note.trim().slice(0, 120) };
  }

  // Who currently claims `host` (presence entries other than `client`, claims not yet expired).
  function claimsOn(presence, host, client = null, now = Date.now()) {
    const out = [];
    for (const p of presence || []) {
      if (!p || (client && p.client === client)) continue;
      for (const c of p.claims || []) if (c.host === host && c.until > now) out.push({ person: p.name, ...c });
    }
    return out;
  }

  return { fuzzy, mentions, isExeTitle, parseClaim, claimsOn };
});
