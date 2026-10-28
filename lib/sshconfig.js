// Minimal OpenSSH client config + known_hosts support, so Nexus resolves hosts the way `ssh` does.
const fs = require('fs');
const os = require('os');
const path = require('path');
const crypto = require('crypto');

const SSH_DIR = path.join(os.homedir(), '.ssh');
const unquote = (s) => s.replace(/^"(.*)"$/, '$1');
const expandHome = (p) => unquote(p).replace(/^~(?=[\\/]|$)/, os.homedir());
const words = (v) => (v.match(/"[^"]+"|\S+/g) || []).map(unquote);

// ---------------------------------------------------------------- ssh_config
// Relative Include paths resolve against the top-level config's directory (~/.ssh), as OpenSSH does.
function readSshConfig(file, depth = 0, out = [], baseDir = path.dirname(file)) {
  if (depth > 8 || !fs.existsSync(file)) return out;
  for (const raw of fs.readFileSync(file, 'utf8').split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith('#')) continue;
    const mm = /^(\S+?)\s*(?:=\s*|\s+)(.*)$/.exec(line);
    if (!mm) continue;
    const key = mm[1].toLowerCase(), value = mm[2].trim();
    if (key === 'include') {
      for (const inc of words(value)) {
        let p = expandHome(inc);
        if (!path.isAbsolute(p)) p = path.join(baseDir, p);
        readSshConfig(p, depth + 1, out, baseDir);
      }
    } else out.push({ key, value });
  }
  return out;
}

function globMatch(pattern, s) {
  const re = new RegExp('^' + pattern.replace(/[.+^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*').replace(/\?/g, '.') + '$', 'i');
  return re.test(s);
}

const patsMatch = (pats, name) => pats.some((p) => !p.startsWith('!') && globMatch(p, name)) && !pats.some((p) => p.startsWith('!') && globMatch(p.slice(1), name));

// Does a "Match" line apply to this alias? Supports: all, host <patterns>, originalhost <patterns>, and "final"
// (treated like all). Criteria Nexus can't evaluate (user, localuser, exec, canonical, tagged) make the block inactive,
// which is the safe reading: options in it are simply not applied.
function matchApplies(criteria, alias, hostname) {
  const w = words(criteria);
  if (!w.length) return false;
  let i = 0, ok = true;
  while (i < w.length) {
    let neg = false, k = w[i].toLowerCase();
    if (k.startsWith('!')) { neg = true; k = k.slice(1); }
    let hit;
    if (k === 'all' || k === 'final' || k === 'canonical') { hit = k === 'all' || k === 'final'; i += 1; }
    else if (k === 'host' || k === 'originalhost') { hit = patsMatch((w[i + 1] || '').split(','), k === 'host' && hostname ? hostname : alias) || patsMatch((w[i + 1] || '').split(','), alias); i += 2; }
    else return false; // user / localuser / exec / tagged: not evaluated
    if (neg) hit = !hit;
    ok = ok && hit;
  }
  return ok;
}

// "user@host:port" (or "host", "[::1]:22") from a ProxyJump value -> { user, host, port }.
function parseJump(spec) {
  const m = /^(?:([^@]+)@)?(\[[^\]]+\]|[^:]+)(?::(\d+))?$/.exec(String(spec).trim());
  if (!m) return null;
  return { user: m[1] || null, host: m[2].replace(/^\[|\]$/g, ''), port: m[3] ? Number(m[3]) : null };
}

// Resolve options for a host alias. First value wins, IdentityFile/UserKnownHostsFile accumulate. jumps: the ProxyJump
// chain (first hop first), each an { user, host, port } to connect through, or [] for a direct connection.
function sshConfigFor(alias, configFile = path.join(SSH_DIR, 'config')) {
  const res = { identityFiles: [], knownHostsFiles: [], jumps: [] };
  let active = true, proxyJump = null;
  for (const { key, value } of readSshConfig(configFile)) {
    if (key === 'host') {
      active = patsMatch(words(value), alias);
      continue;
    }
    if (key === 'match') { active = matchApplies(value, alias, res.hostname); continue; }
    if (!active) continue;
    if (key === 'identityfile') res.identityFiles.push(expandHome(value));
    else if (key === 'userknownhostsfile') res.knownHostsFiles.push(...words(value).map(expandHome));
    else if (key === 'hostname' && !res.hostname) res.hostname = value;
    else if (key === 'user' && !res.user) res.user = value;
    else if (key === 'port' && !res.port) res.port = Number(value);
    else if (key === 'proxyjump' && proxyJump == null) proxyJump = value;
  }
  if (proxyJump && proxyJump.toLowerCase() !== 'none') res.jumps = proxyJump.split(',').map(parseJump).filter(Boolean);
  return res;
}

// ---------------------------------------------------------------- known_hosts
// The host key arrives as an SSH wire-format blob: string keytype, then key data.
function blobType(blob) {
  const len = blob.readUInt32BE(0);
  return blob.subarray(4, 4 + len).toString();
}

function hostFieldMatches(field, names) {
  let hit = false;
  for (const pat of field.split(',')) {
    if (pat.startsWith('|1|')) {
      const [, , salt, hash] = pat.split('|');
      if (names.some((n) => crypto.createHmac('sha1', Buffer.from(salt, 'base64')).update(n).digest('base64') === hash)) hit = true;
    } else if (pat.startsWith('!')) {
      if (names.some((n) => globMatch(pat.slice(1), n))) return false;
    } else if (names.some((n) => globMatch(pat, n))) hit = true;
  }
  return hit;
}

// -> 'match' | 'mismatch' (same key type, different key: possible MITM) | 'revoked' | 'unknown'
function checkKnownHosts(files, host, port, blob) {
  const names = Number(port) === 22 ? [host] : [`[${host}]:${port}`];
  const type = blobType(blob);
  const b64 = blob.toString('base64');
  let mismatch = false, match = false;
  for (const file of files) {
    if (!fs.existsSync(file)) continue;
    for (const raw of fs.readFileSync(file, 'utf8').split(/\r?\n/)) {
      const line = raw.trim();
      if (!line || line.startsWith('#')) continue;
      let parts = line.split(/\s+/);
      let marker = null;
      if (parts[0].startsWith('@')) { marker = parts[0]; parts = parts.slice(1); }
      if (marker === '@cert-authority' || parts.length < 3) continue;
      const [hosts, ktype, key] = parts;
      if (marker === '@revoked') { if (key === b64) return 'revoked'; continue; }
      if (!hostFieldMatches(hosts, names)) continue;
      if (key === b64) match = true;
      else if (ktype === type) mismatch = true;
    }
  }
  // A revoked key always loses (checked above, wherever it appears); an exact match beats a stale entry.
  return match ? 'match' : mismatch ? 'mismatch' : 'unknown';
}

const fingerprint = (blob) => 'SHA256:' + crypto.createHash('sha256').update(blob).digest('base64').replace(/=+$/, '');

module.exports = { SSH_DIR, expandHome, readSshConfig, sshConfigFor, globMatch, matchApplies, parseJump, checkKnownHosts, blobType, fingerprint };
