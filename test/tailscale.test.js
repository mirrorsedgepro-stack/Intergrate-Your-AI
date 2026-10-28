const test = require('node:test');
const assert = require('node:assert');
const { planPolicy, guestsIn, encodeInvite, decodeInvite, isTailnetAddress, GUEST_GROUP, SPARK_TAG } = require('../lib/tailscale');

// What a fresh tailnet ships with: everyone reaches everything, SSH to own devices in check mode.
const DEFAULT = {
  grants: [{ src: ['*'], dst: ['*'], ip: ['*'] }],
  ssh: [{ action: 'check', src: ['autogroup:member'], dst: ['autogroup:self'], users: ['autogroup:nonroot', 'root'] }],
};

test('planPolicy locks guests to SSH on the Sparks and keeps the owner fully connected', () => {
  const { policy, changes } = planPolicy(DEFAULT, { guests: ['Friend@Example.com'], sshUsers: ['jcee', 'jcee-slave'] });
  assert.ok(changes.length >= 5);
  assert.deepStrictEqual(policy.groups[GUEST_GROUP], ['friend@example.com']);
  assert.deepStrictEqual(policy.tagOwners[SPARK_TAG], ['autogroup:admin']);
  // the allow-all grant is narrowed to admins: no rule lets guests reach "*"
  for (const g of policy.grants) if (g.dst.includes('*')) assert.deepStrictEqual(g.src, ['autogroup:admin']);
  assert.ok(policy.grants.some((g) => g.src[0] === GUEST_GROUP && g.dst[0] === SPARK_TAG && g.ip[0] === 'tcp:22'));
  assert.ok(policy.grants.some((g) => g.src[0] === 'autogroup:member' && g.dst[0] === 'autogroup:self'));
  const rule = policy.ssh.find((r) => r.dst[0] === SPARK_TAG);
  assert.deepStrictEqual(rule.users, ['jcee', 'jcee-slave']);
  assert.strictEqual(rule.action, 'accept');
  // untouched: the existing self-SSH rule
  assert.ok(policy.ssh.some((r) => r.action === 'check' && r.dst[0] === 'autogroup:self'));
  assert.deepStrictEqual(DEFAULT.grants[0].src, ['*'], 'input is not mutated');
});

test('planPolicy is idempotent and only reports real changes', () => {
  const first = planPolicy(DEFAULT, { guests: ['a@x.com'], sshUsers: ['jcee'] }).policy;
  const again = planPolicy(first, { guests: ['a@x.com'], sshUsers: ['jcee'] });
  assert.deepStrictEqual(again.changes, []);
  assert.deepStrictEqual(again.policy, first);
  const more = planPolicy(first, { guests: ['a@x.com', 'b@x.com'], sshUsers: ['jcee'] });
  assert.deepStrictEqual(more.changes.length, 1);
  assert.deepStrictEqual(guestsIn(more.policy), ['a@x.com', 'b@x.com']);
});

test('planPolicy narrows legacy allow-all ACLs too', () => {
  const legacy = { acls: [{ action: 'accept', src: ['*'], dst: ['*:*'] }, { action: 'accept', src: ['group:ops'], dst: ['tag:db:5432'] }] };
  const { policy } = planPolicy(legacy, { sshUsers: ['jcee'] });
  assert.deepStrictEqual(policy.acls[0].src, ['autogroup:admin']);
  assert.deepStrictEqual(policy.acls[1].src, ['group:ops']);
});

test('invite codes round-trip and reject junk', () => {
  const code = encodeInvite({ tailnet: 'x.ts.net', machines: [{ name: 'Spark 1', host: 'spark1-aresj.tail1.ts.net', user: 'jcee', icon: 'nvidia' }] });
  assert.match(code, /^nexus-invite:/);
  const back = decodeInvite(code);
  assert.strictEqual(back.machines[0].host, 'spark1-aresj.tail1.ts.net');
  assert.strictEqual(back.machines[0].user, 'jcee');
  assert.throws(() => decodeInvite('hello'));
  const evil = encodeInvite({ machines: [{ name: 'x', host: 'a; rm -rf ~', user: 'u$(id)' }, { host: 'ok.ts.net', user: 'jcee' }] });
  assert.deepStrictEqual(decodeInvite(evil).machines.map((m) => m.host), ['ok.ts.net']);
});

test('invite codes: found inside a pasted message, even line-wrapped; damaged ones get a clear error', () => {
  const code = encodeInvite({ tailnet: 'x.ts.net', machines: [{ name: 'Spark 1', host: 'spark1.tail1.ts.net', user: 'jcee' }] });
  const wrapped = code.slice(0, 30) + '\r\n   ' + code.slice(30);
  const message = `3. In Nexus click Share, paste this invite code:\n   ${wrapped}\n\nYou'll log in as Spark 1: jcee.`;
  assert.strictEqual(decodeInvite(message).machines[0].host, 'spark1.tail1.ts.net');
  assert.throws(() => decodeInvite('nexus-invite:garbage'), /incomplete or damaged/);
  assert.throws(() => decodeInvite(code.slice(0, 40)), /incomplete or damaged/);
  assert.throws(() => decodeInvite(encodeInvite({ machines: [] })), /no machines/);
});

test('isTailnetAddress', () => {
  assert.ok(isTailnetAddress('100.101.2.3'));
  assert.ok(isTailnetAddress('spark1.tail1234.ts.net'));
  assert.ok(!isTailnetAddress('192.168.8.236'));
  assert.ok(!isTailnetAddress('100.200.1.1'));
});

test('invite codes only accept tailnet addresses', () => {
  const code = encodeInvite({ machines: [{ name: 'evil', host: 'attacker.example.com', user: 'x' }, { name: 'ip', host: '203.0.113.5', user: 'x' }, { name: 'ok', host: '100.101.2.3', user: 'jcee' }] });
  assert.deepStrictEqual(decodeInvite(code).machines.map((m) => m.host), ['100.101.2.3']);
  assert.throws(() => decodeInvite(encodeInvite({ machines: [{ host: 'lan-box.local', user: 'x' }] })), /Tailscale/);
});

test('matchDevices: tailnet name first, ambiguous hostnames and this PC are never matched', () => {
  const { matchDevices } = require('../lib/tailscale');
  const devices = [
    { id: 'pc', name: 'my-pc.tail1.ts.net', hostname: 'spark1', tags: [] },
    { id: 's1', name: 'spark1.tail1.ts.net', hostname: 'spark1', tags: [] },
    { id: 'a', name: 'dup-a.tail1.ts.net', hostname: 'dup', tags: [] },
    { id: 'b', name: 'dup-b.tail1.ts.net', hostname: 'dup', tags: [] },
  ];
  const [byName, byHost, dup, spoof] = matchDevices(devices, [
    { machineId: 'm1', hostname: 'whatever', tsHost: 'spark1.tail1.ts.net' },
    { machineId: 'm2', hostname: 'spark1' },
    { machineId: 'm3', hostname: 'dup' },
    { machineId: 'm4', hostname: 'my-pc', tsHost: 'my-pc.tail1.ts.net' },
  ], 'my-pc.tail1.ts.net.');
  assert.strictEqual(byName.device.id, 's1');
  assert.strictEqual(byHost.device.id, 's1', 'this PC (same hostname) is excluded, leaving one match');
  assert.strictEqual(dup.device, null);
  assert.strictEqual(dup.ambiguous, true);
  assert.strictEqual(spoof.device, null, 'never tag this PC');
});

test('planPolicy per-guest mode: admins keep the shared rule, each guest gets their own account, removed guests lose theirs', () => {
  const { guestUser } = require('../lib/tailscale');
  assert.strictEqual(guestUser('Alice.Smith+gpu@example.com'), 'nexus-alice-smith-gpu');
  const first = planPolicy(DEFAULT, { guests: ['alice@x.com', 'bob@x.com'], sshUsers: ['jcee'], perGuestUsers: true });
  const shared = first.policy.ssh.find((r) => r.dst[0] === SPARK_TAG && r.src.length === 1 && r.src[0] === 'autogroup:admin');
  assert.deepStrictEqual(shared.users, ['jcee']);
  assert.ok(!first.policy.ssh.some((r) => r.src.includes(GUEST_GROUP)), 'guests do not get the shared account');
  const alice = first.policy.ssh.find((r) => r.src[0] === 'alice@x.com');
  assert.deepStrictEqual(alice.users, ['nexus-alice']);
  // idempotent
  assert.deepStrictEqual(planPolicy(first.policy, { guests: ['alice@x.com', 'bob@x.com'], sshUsers: ['jcee'], perGuestUsers: true }).changes, []);
  // removing bob drops his rule
  const less = planPolicy(first.policy, { guests: ['alice@x.com'], sshUsers: ['jcee'], perGuestUsers: true });
  assert.ok(!less.policy.ssh.some((r) => r.src[0] === 'bob@x.com'));
  assert.ok(less.changes.some((c) => /remove bob@x.com/.test(c)));
  // switching an existing shared-mode policy to per-guest narrows the legacy rule
  const sharedMode = planPolicy(DEFAULT, { guests: ['alice@x.com'], sshUsers: ['jcee'] }).policy;
  const moved = planPolicy(sharedMode, { guests: ['alice@x.com'], sshUsers: ['jcee'], perGuestUsers: true });
  assert.ok(!moved.policy.ssh.some((r) => r.src.includes(GUEST_GROUP) && r.dst[0] === SPARK_TAG));
  assert.ok(moved.changes.some((c) => /no longer share/.test(c)));
});

// Credentials are assembled at run time so no string in this file looks like a real Tailscale key to secret scanners.
const K = (kind, id, rest) => ['tskey', kind, id, rest].filter(Boolean).join('-');

test('credentials: API tokens and OAuth client secrets are told apart; anything else is refused', () => {
  const { parseCredential } = require('../lib/tailscale');
  const api = K('api', 'kFAKE0CNTRL', 'xyz'), client = K('client', 'kFAKE0CNTRL', 'secretPart');
  assert.deepEqual(parseCredential(` ${api} `), { kind: 'api', token: api });
  assert.deepEqual(parseCredential(client), { kind: 'oauth', clientId: null, secret: client });
  assert.deepEqual(parseCredential(`kFAKE0CNTRL:${client}`), { kind: 'oauth', clientId: 'kFAKE0CNTRL', secret: client });
  for (const bad of ['', K('auth', 'abc'), `Bearer ${api}`, `${api} y`, null]) assert.equal(parseCredential(bad), null, String(bad));
});

test('OAuth clients exchange the secret for an access token once per hour', async () => {
  const { TailscaleApi } = require('../lib/tailscale');
  const fresh = K('api', 'fresh'), mine = K('api', 'mine'), secret = K('client', 'secret');
  const calls = [];
  const realFetch = global.fetch;
  global.fetch = async (url, opts = {}) => {
    calls.push({ url, auth: (opts.headers || {}).Authorization, body: opts.body && opts.body.toString() });
    if (String(url).endsWith('/oauth/token')) return { ok: true, status: 200, text: async () => JSON.stringify({ access_token: fresh, expires_in: 3600 }) };
    return { ok: true, status: 200, statusText: 'OK', text: async () => JSON.stringify({ devices: [{ id: 1 }] }), headers: new Map() };
  };
  try {
    const api = new TailscaleApi(`kID:${secret}`);
    assert.equal((await api.devices()).length, 1);
    await api.devices();
    assert.equal(calls.filter((c) => String(c.url).endsWith('/oauth/token')).length, 1, 'one exchange serves both calls');
    assert.ok(calls[0].body.includes(`client_secret=${secret}`));
    assert.ok(calls[0].body.includes('client_id=kID'));
    assert.equal(calls[1].auth, `Bearer ${fresh}`);
    const plain = new TailscaleApi(mine);
    await plain.devices();
    assert.equal(calls[calls.length - 1].auth, `Bearer ${mine}`, 'API tokens are used as-is');
  } finally { global.fetch = realFetch; }
});
