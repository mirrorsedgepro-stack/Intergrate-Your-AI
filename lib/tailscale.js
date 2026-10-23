// Tailscale integration: the local client (status/peers), the Tailscale API (invites, policy, tags), and the
// pure policy/invite-code helpers. Guests are invited to the owner's tailnet and reach only the tagged Sparks,
// over Tailscale SSH, as the configured Linux users.
const { execFile } = require('child_process');
const fs = require('fs');
const path = require('path');

const API = 'https://api.tailscale.com/api/v2';
const GUEST_GROUP = 'group:nexus-guests';
const SPARK_TAG = 'tag:nexus-spark';
const INVITE_PREFIX = 'nexus-invite:';

// ---------------------------------------------------------------- local client
function cliPath() {
  if (process.platform === 'darwin') {
    const app = '/Applications/Tailscale.app/Contents/MacOS/Tailscale';
    return fs.existsSync(app) ? app : 'tailscale';
  }
  if (process.platform !== 'win32') return 'tailscale';
  const p = path.join(process.env.ProgramFiles || 'C:\\Program Files', 'Tailscale', 'tailscale.exe');
  return fs.existsSync(p) ? p : 'tailscale';
}

function localStatus() {
  return new Promise((resolve) => {
    execFile(cliPath(), ['status', '--json'], { windowsHide: true, timeout: 8000 }, (err, stdout, stderr) => {
      if (err && err.code === 'ENOENT') return resolve({ installed: false });
      let data;
      try { data = JSON.parse(stdout); } catch { return resolve({ installed: true, running: false, error: (stderr || String(err || '')).trim() }); }
      const peer = (p) => ({
        hostName: p.HostName,
        dnsName: String(p.DNSName || '').replace(/\.$/, ''),
        ips: p.TailscaleIPs || [],
        online: !!p.Online,
        os: p.OS,
        tags: p.Tags || [],
      });
      resolve({
        installed: true,
        running: true,
        state: data.BackendState, // Running | NeedsLogin | Stopped …
        tailnet: (data.CurrentTailnet && data.CurrentTailnet.Name) || null,
        magicDNSSuffix: (data.CurrentTailnet && data.CurrentTailnet.MagicDNSSuffix) || data.MagicDNSSuffix || null,
        self: data.Self ? peer(data.Self) : null,
        // The signed-in Tailscale user (used as the default name in team chat).
        user: (() => { const u = data.Self && data.User && data.User[data.Self.UserID]; return u ? { login: u.LoginName, name: u.DisplayName } : null; })(),
        peers: Object.values(data.Peer || {}).map(peer),
      });
    });
  });
}

// ---------------------------------------------------------------- API client
class TailscaleApi {
  constructor(token, tailnet = '-') {
    this.token = token;
    this.tailnet = tailnet;
  }

  async req(method, p, { body, headers = {}, raw = false } = {}) {
    const res = await fetch(API + p, {
      method,
      headers: { Authorization: `Bearer ${this.token}`, ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}), ...headers },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
    const text = await res.text();
    let json = null;
    try { json = text ? JSON.parse(text) : null; } catch { /* non-JSON body */ }
    if (!res.ok) {
      const msg = (json && (json.message || (json.data && JSON.stringify(json.data)))) || text || res.statusText;
      const e = new Error(`Tailscale API ${res.status}: ${msg}`);
      e.status = res.status;
      throw e;
    }
    return raw ? { json, headers: res.headers } : json;
  }

  t(p) { return `/tailnet/${encodeURIComponent(this.tailnet)}${p}`; }
  async devices() { return (await this.req('GET', this.t('/devices'))).devices || []; }
  async users() { return (await this.req('GET', this.t('/users?type=all'))).users || []; }
  async invites() { return (await this.req('GET', this.t('/user-invites'))) || []; }
  async createInvite(email) { return (await this.req('POST', this.t('/user-invites'), { body: [{ role: 'member', ...(email ? { email } : {}) }] }))[0]; }
  deleteInvite(id) { return this.req('DELETE', `/user-invites/${encodeURIComponent(id)}`); }
  deleteUser(id) { return this.req('POST', `/users/${encodeURIComponent(id)}/delete`); }
  setTags(deviceId, tags) { return this.req('POST', `/device/${encodeURIComponent(deviceId)}/tags`, { body: { tags } }); }

  async getPolicy() {
    const { json, headers } = await this.req('GET', this.t('/acl'), { headers: { Accept: 'application/json' }, raw: true });
    return { policy: json || {}, etag: headers.get('etag') };
  }
  validatePolicy(policy) { return this.req('POST', this.t('/acl/validate'), { body: policy }); }
  setPolicy(policy, etag) {
    return this.req('POST', this.t('/acl'), { body: policy, headers: { Accept: 'application/json', ...(etag ? { 'If-Match': etag } : {}) } });
  }
}

// ---------------------------------------------------------------- policy planning (pure)
const has = (list, v) => Array.isArray(list) && list.includes(v);
const isAllowAllSrc = (src) => has(src, '*') || has(src, 'autogroup:member');
const sameSet = (a, b) => JSON.stringify([...(a || [])].sort()) === JSON.stringify([...(b || [])].sort());

// Returns { policy, changes } where policy lets the owner (admins) keep full access, gives guests SSH on the tagged
// Sparks only (as sshUsers), and turns any "everyone can reach everything" rule into an admins-only rule so that
// invited guests can't reach the owner's other devices. Idempotent: planning twice yields no further changes.
function planPolicy(current, { guests = [], sshUsers = [] } = {}) {
  const p = JSON.parse(JSON.stringify(current || {}));
  const changes = [];

  const groups = (p.groups = p.groups || {});
  const wanted = [...new Set(guests.map((g) => g.trim().toLowerCase()).filter(Boolean))].sort();
  if (!sameSet(groups[GUEST_GROUP], wanted)) {
    changes.push(`Guest group ${GUEST_GROUP}: ${wanted.length ? wanted.join(', ') : '(no guests yet)'}`);
    groups[GUEST_GROUP] = wanted;
  }

  const owners = (p.tagOwners = p.tagOwners || {});
  if (!sameSet(owners[SPARK_TAG], ['autogroup:admin'])) {
    owners[SPARK_TAG] = ['autogroup:admin'];
    changes.push(`Create tag ${SPARK_TAG} (owned by admins) for the Sparks`);
  }

  // Narrow allow-all rules (legacy "acls" and "grants") to admins.
  let narrowed = false;
  for (const rule of p.acls || []) {
    const srcKey = rule.src ? 'src' : 'users';
    const dst = rule.dst || rule.ports || [];
    if (rule.action === 'accept' && isAllowAllSrc(rule[srcKey]) && dst.some((d) => d === '*:*' || d === '*')) {
      rule[srcKey] = ['autogroup:admin'];
      narrowed = true;
    }
  }
  for (const g of p.grants || []) {
    if (isAllowAllSrc(g.src) && has(g.dst, '*')) { g.src = ['autogroup:admin']; narrowed = true; }
  }
  if (narrowed) changes.push('Limit the "everyone can reach everything" rule to admins (you), so guests only see the shared machines');

  const grants = (p.grants = p.grants || []);
  const ensureGrant = (g, why) => {
    if (grants.some((x) => sameSet(x.src, g.src) && sameSet(x.dst, g.dst) && sameSet(x.ip, g.ip))) return;
    grants.push(g);
    changes.push(why);
  };
  if (narrowed) ensureGrant({ src: ['autogroup:member'], dst: ['autogroup:self'], ip: ['*'] }, 'Members can still reach their own devices');
  ensureGrant({ src: ['autogroup:admin'], dst: [SPARK_TAG], ip: ['*'] }, 'Admins (you) keep full access to the Sparks');
  ensureGrant({ src: [GUEST_GROUP], dst: [SPARK_TAG], ip: ['tcp:22'] }, 'Guests can reach the Sparks on SSH (port 22) only');

  const ssh = (p.ssh = p.ssh || []);
  const users = [...new Set(sshUsers.filter(Boolean))].sort();
  const src = ['autogroup:admin', GUEST_GROUP];
  const existing = ssh.find((r) => sameSet(r.src, src) && sameSet(r.dst, [SPARK_TAG]));
  if (!existing) {
    ssh.push({ action: 'accept', src, dst: [SPARK_TAG], users });
    changes.push(`Tailscale SSH: you and guests log in to the shared machines as ${users.join(' / ') || '(no users)'}`);
  } else if (!sameSet(existing.users, users) || existing.action !== 'accept') {
    existing.users = users;
    existing.action = 'accept';
    changes.push(`Tailscale SSH users on the shared machines: ${users.join(' / ')}`);
  }
  return { policy: p, changes };
}

const guestsIn = (policy) => [...((policy && policy.groups && policy.groups[GUEST_GROUP]) || [])];

// ---------------------------------------------------------------- invite codes (pure)
// A guest pastes this into Nexus to add the Sparks (tailnet addresses + login users). No secrets inside.
function encodeInvite(data) {
  return INVITE_PREFIX + Buffer.from(JSON.stringify({ v: 1, ...data })).toString('base64url');
}
// Accepts the bare code or the whole invitation message pasted, even if a mail client wrapped the code's line.
function decodeInvite(code) {
  const text = String(code || '');
  const at = text.indexOf(INVITE_PREFIX);
  if (at < 0) throw new Error('Not a Nexus invite code (it starts with nexus-invite:)');
  // A wrapped code continues on the next line(s); words after it don't belong to it. Only the right number of pieces
  // decodes to valid JSON, so try 1, 2, 3… pieces.
  const pieces = (/^[A-Za-z0-9_-]+(?:\s+[A-Za-z0-9_-]+)*/.exec(text.slice(at + INVITE_PREFIX.length)) || [''])[0].split(/\s+/);
  let data = null;
  for (let n = 1; n <= Math.min(pieces.length, 12) && !data; n++) {
    try { data = JSON.parse(Buffer.from(pieces.slice(0, n).join(''), 'base64url').toString('utf8')); } catch { /* try more pieces */ }
  }
  if (!data || typeof data !== 'object') throw new Error('That invite code is incomplete or damaged. Copy it again from the invitation');
  if (data.v !== 1 || !Array.isArray(data.machines)) throw new Error('Unsupported invite code');
  // Only tailnet addresses: an invite can't point Nexus at an arbitrary internet host (which would then receive your
  // SSH login attempts, terminal sessions, team-chat presence and anything you paste or drop into those panes).
  data.machines = data.machines
    .filter((m) => m && typeof m.host === 'string' && /^[\w.-]+$/.test(m.host) && isTailnetAddress(m.host))
    .map((m) => ({ name: String(m.name || m.host).slice(0, 40), host: m.host, user: String(m.user || '').replace(/[^\w.-]/g, ''), icon: m.icon === 'nvidia' ? 'nvidia' : '' }));
  if (!data.machines.length) throw new Error('That invite code has no machines on a Tailscale network in it');
  return data;
}

// Match configured SSH machines to tailnet devices. Prefer the machine's recorded Tailscale name (exact); fall back to
// the hostname the machine reports only when exactly one device has it; never pick this PC itself. Tagging a device
// gives guests SSH to it, so a machine that renames itself to match another device must not get that device tagged.
function matchDevices(devices, machines, selfDnsName = '') {
  const norm = (s) => String(s || '').toLowerCase().replace(/\.$/, '');
  const self = norm(selfDnsName);
  const pool = (devices || []).filter((d) => !self || norm(d.name) !== self);
  return (machines || []).map((m) => {
    let d = m.tsHost ? pool.find((x) => norm(x.name) === norm(m.tsHost) || (x.addresses || []).includes(m.tsHost)) : null;
    let ambiguous = false;
    if (!d && m.hostname) {
      const hits = pool.filter((x) => norm(x.hostname) === norm(m.hostname));
      if (hits.length === 1) [d] = hits;
      else ambiguous = hits.length > 1;
    }
    return {
      machineId: m.machineId,
      hostname: m.hostname,
      ambiguous,
      device: d ? { id: d.id || d.nodeId, name: d.name, addresses: d.addresses || [], tags: d.tags || [], online: !!d.connectedToControl, lastSeen: d.lastSeen } : null,
    };
  });
}

const isTailnetAddress = (h) => /\.ts\.net$/i.test(h || '') || /^100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7])\./.test(h || '');

module.exports = { localStatus, TailscaleApi, planPolicy, guestsIn, encodeInvite, decodeInvite, isTailnetAddress, matchDevices, GUEST_GROUP, SPARK_TAG };
