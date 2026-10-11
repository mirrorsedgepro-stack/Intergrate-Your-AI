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
