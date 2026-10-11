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

  return { fuzzy, mentions, isExeTitle };
});
