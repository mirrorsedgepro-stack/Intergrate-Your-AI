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

// ---------------------------------------------------------------- credentials
// Two kinds of credential work with the API:
//   tskey-api-…                     a personal API access token: carries the owner's full admin rights, expires ≤ 90 days.
//   [client_id:]tskey-client-…      an OAuth client secret: scoped to what Nexus needs (devices:core on tag:nexus-spark,
//                                   policy_file, users), never expires, and is exchanged for 1-hour access tokens.
// The OAuth client is the recommended choice: a leaked config.json then gives an attacker Nexus's permissions, not yours.
const OAUTH_SCOPES = ['devices:core', 'devices:core:read', 'devices:posture_attributes', 'policy_file', 'users'];

function parseCredential(raw) {
  const t = String(raw || '').trim();
  if (/^tskey-api-[\w-]+$/.test(t)) return { kind: 'api', token: t };
  const m = /^(?:([\w-]+):)?(tskey-client-[\w-]+)$/.exec(t);
  if (m) return { kind: 'oauth', clientId: m[1] || null, secret: m[2] };
  return null;
}

// ---------------------------------------------------------------- API client
class TailscaleApi {
  // cred: a credential string (see parseCredential) or a parsed one.
  constructor(cred, tailnet = '-') {
    this.cred = typeof cred === 'string' ? parseCredential(cred) || { kind: 'api', token: cred } : cred;
    this.tailnet = tailnet;
    this.access = null; // { token, exp } for OAuth clients
  }

  // Bearer token for the next request: the API token itself, or a fresh OAuth access token (renewed 2 min early).
  async bearer() {
    if (this.cred.kind !== 'oauth') return this.cred.token;
    if (this.access && this.access.exp - Date.now() > 120000) return this.access.token;
    const form = new URLSearchParams({ client_secret: this.cred.secret });
    if (this.cred.clientId) form.set('client_id', this.cred.clientId);
    const res = await fetch(`${API}/oauth/token`, { method: 'POST', headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: form });
    const text = await res.text();
    let json = null;
    try { json = text ? JSON.parse(text) : null; } catch { /* non-JSON body */ }
    if (!res.ok || !json || !json.access_token) {
      const msg = (json && (json.message || json.error_description || json.error)) || text || res.statusText;
      throw new Error(`Tailscale rejected the OAuth client (${res.status}): ${msg}. Check the client ID/secret and that it has the scopes ${OAUTH_SCOPES.join(', ')}`);
    }
    this.access = { token: json.access_token, exp: Date.now() + (Number(json.expires_in) || 3600) * 1000 };
    return this.access.token;
  }

  async req(method, p, { body, headers = {}, raw = false } = {}) {
    const res = await fetch(API + p, {
      method,
      headers: { Authorization: `Bearer ${await this.bearer()}`, ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}), ...headers },
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
function planPolicy(current, { guests = [], sshUsers = [], perGuestUsers = false } = {}) {
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

  // Narrow every rule whose source is "everyone" (legacy "acls" and "grants") to admins. Guests become members of the
  // tailnet, so any rule with src * / autogroup:member reaches them: not only "*:*", but also "*:22", "tag:prod:*" and
  // the like. The only everyone-rule that is safe to keep is "members reach their own devices" (autogroup:self).
  let narrowed = false;
  const selfOnly = (dst) => dst.length > 0 && dst.every((d) => /^autogroup:self(:|$)/.test(String(d)));
  const describe = (dst) => dst.map(String).join(', ') || '(no destination)';
  for (const rule of p.acls || []) {
    const srcKey = rule.src ? 'src' : 'users';
    const dst = rule.dst || rule.ports || [];
    if (rule.action === 'accept' && isAllowAllSrc(rule[srcKey]) && !selfOnly(dst)) {
      rule[srcKey] = ['autogroup:admin'];
      narrowed = true;
      changes.push(`Limit the "everyone → ${describe(dst)}" rule to admins (you), so guests can't use it`);
    }
  }
  for (const g of p.grants || []) {
    if (isAllowAllSrc(g.src) && !selfOnly(g.dst || [])) {
      g.src = ['autogroup:admin'];
      narrowed = true;
      changes.push(`Limit the "everyone → ${describe(g.dst || [])}" grant to admins (you), so guests can't use it`);
    }
  }

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
  // Shared-account mode (the original): one rule lets admins and guests in as the machines' configured users.
  // Per-guest mode: admins keep that rule for themselves; each guest gets a rule for their own Linux account only,
  // so a guest can't read the owner's files, agent sign-ins or tmux sessions, and removing them removes real access.
  const sharedSrc = perGuestUsers ? ['autogroup:admin'] : ['autogroup:admin', GUEST_GROUP];
  const legacySrc = ['autogroup:admin', GUEST_GROUP];
  const legacy = ssh.find((r) => sameSet(r.src, legacySrc) && sameSet(r.dst, [SPARK_TAG]));
  if (perGuestUsers && legacy) {
    legacy.src = ['autogroup:admin'];
    changes.push(`Tailscale SSH: guests no longer share your account (${users.join(' / ')}); each gets their own`);
  }
  const existing = ssh.find((r) => sameSet(r.src, sharedSrc) && sameSet(r.dst, [SPARK_TAG]));
  if (!existing) {
    ssh.push({ action: 'accept', src: sharedSrc, dst: [SPARK_TAG], users });
    changes.push(perGuestUsers ? `Tailscale SSH: you log in to the shared machines as ${users.join(' / ') || '(no users)'}` : `Tailscale SSH: you and guests log in to the shared machines as ${users.join(' / ') || '(no users)'}`);
  } else if (!sameSet(existing.users, users) || existing.action !== 'accept') {
    existing.users = users;
    existing.action = 'accept';
    changes.push(`Tailscale SSH users on the shared machines: ${users.join(' / ')}`);
  }
  if (perGuestUsers) {
    // One rule per guest: src [email] -> dst [tag] as [their account]. Rules for guests no longer in the group go.
    const isGuestRule = (r) => Array.isArray(r.src) && r.src.length === 1 && /@/.test(r.src[0]) && sameSet(r.dst, [SPARK_TAG]);
    for (const email of wanted) {
      const u = guestUser(email);
      const r = ssh.find((x) => isGuestRule(x) && x.src[0].toLowerCase() === email);
      if (!r) { ssh.push({ action: 'accept', src: [email], dst: [SPARK_TAG], users: [u] }); changes.push(`Tailscale SSH: ${email} logs in to the shared machines as ${u}`); }
      else if (!sameSet(r.users, [u]) || r.action !== 'accept') { r.users = [u]; r.action = 'accept'; changes.push(`Tailscale SSH: ${email} logs in as ${u}`); }
    }
    for (let i = ssh.length - 1; i >= 0; i--) {
      const r = ssh[i];
      if (isGuestRule(r) && !wanted.includes(r.src[0].toLowerCase())) { ssh.splice(i, 1); changes.push(`Tailscale SSH: remove ${r.src[0]}'s login`); }
    }
  }
  return { policy: p, changes };
}

// The Linux account a guest gets in per-guest mode: nexus-<local part of their email>, shortened and sanitised so it
// is a valid user name everywhere (lower-case, [a-z0-9-], 32 chars). Collisions between e.g. a.b@ and a_b@ are
// possible but harmless: Tailscale SSH still binds each account to one email.
function guestUser(email) {
  const local = String(email || '').toLowerCase().split('@')[0].replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 24) || 'guest';
  return `nexus-${local}`;
}

const guestsIn = (policy) => [...((policy && policy.groups && policy.groups[GUEST_GROUP]) || [])];

// ---------------------------------------------------------------- invite codes (pure)
// A guest pastes this into Nexus to add the Sparks (tailnet addresses + login users). No secrets inside.
function encodeInvite(data) {
  return INVITE_PREFIX + Buffer.from(JSON.stringify({ v: 1, ...data })).toString('base64url');
}
// Accepts the bare code or the whole invitation message pasted, even if a mail client wrapped the code's line.
// localTailnet (optional): the tailnet this Nexus is signed in to. A code written for another tailnet is refused, so a
// guest who is already on some other tailnet (work, family) can't be handed a code that points Nexus at a host there.
function decodeInvite(code, localTailnet = null) {
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
  data.tailnet = typeof data.tailnet === 'string' ? data.tailnet.trim().toLowerCase() : '';
  if (localTailnet && data.tailnet && data.tailnet !== String(localTailnet).trim().toLowerCase()) {
    throw new Error(`This invite is for the tailnet "${data.tailnet}", but this computer is signed in to "${localTailnet}". Accept the Tailscale invite for that tailnet first`);
  }
  // Only tailnet addresses: an invite can't point Nexus at an arbitrary internet host (which would then receive your
  // SSH login attempts, terminal sessions, team-chat presence and anything you paste or drop into those panes).
  // Hosts start with a word character so they can never be read as an ssh option (-oProxyCommand=…).
  data.machines = data.machines
    .filter((m) => m && typeof m.host === 'string' && /^\w[\w.-]*$/.test(m.host) && isTailnetAddress(m.host))
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

module.exports = { localStatus, TailscaleApi, parseCredential, OAUTH_SCOPES, planPolicy, guestsIn, guestUser, encodeInvite, decodeInvite, isTailnetAddress, matchDevices, GUEST_GROUP, SPARK_TAG };
