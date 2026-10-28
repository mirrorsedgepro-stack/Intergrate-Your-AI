const test = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const crypto = require('crypto');
const { sshConfigFor, checkKnownHosts, blobType, fingerprint } = require('../lib/sshconfig');

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'nexus-test-'));
const write = (name, text) => { const p = path.join(tmp, name); fs.mkdirSync(path.dirname(p), { recursive: true }); fs.writeFileSync(p, text); return p; };

// SSH wire-format public key blob: string type, then opaque key bytes.
function blob(type, bytes = crypto.randomBytes(32)) {
  const t = Buffer.from(type);
  const len = (n) => { const b = Buffer.alloc(4); b.writeUInt32BE(n); return b; };
  return Buffer.concat([len(t.length), t, len(bytes.length), bytes]);
}

test('ssh_config: Include, first value wins, IdentityFile accumulates', () => {
  write('vendor/conf', 'Host 10.0.0.5\n  User vendor\n  IdentityFile ~/vendor.key\n  UserKnownHostsFile ~/vendor_known\n');
  const cfg = write('config', [
    `Include "${path.join(tmp, 'vendor', 'conf')}"`,
    'Host 10.0.0.5',
    '  User override-ignored',
    '  IdentityFile ~/.ssh/second',
    'Host box',
    '  HostName 10.0.0.9',
    '  Port 2222',
    'Host *',
    '  User fallback',
  ].join('\n'));
  const a = sshConfigFor('10.0.0.5', cfg);
  assert.strictEqual(a.user, 'vendor');
  assert.deepStrictEqual(a.identityFiles.map((f) => path.basename(f)), ['vendor.key', 'second']);
  assert.strictEqual(path.basename(a.knownHostsFiles[0]), 'vendor_known');
  const b = sshConfigFor('box', cfg);
  assert.strictEqual(b.hostname, '10.0.0.9');
  assert.strictEqual(b.port, 2222);
  assert.strictEqual(b.user, 'fallback');
  assert.deepStrictEqual(sshConfigFor('other', cfg).identityFiles, []);
});

test('ssh_config: negated patterns exclude a host', () => {
  const cfg = write('config-neg', 'Host * !secret\n  User everyone\n');
  assert.strictEqual(sshConfigFor('a', cfg).user, 'everyone');
  assert.strictEqual(sshConfigFor('secret', cfg).user, undefined);
});

test('known_hosts: plain, hashed, non-standard port, mismatch, revoked', () => {
  const good = blob('ssh-ed25519');
  const other = blob('ssh-ed25519');
  const rsa = blob('ssh-rsa');
  const salt = crypto.randomBytes(20);
  const hashed = `|1|${salt.toString('base64')}|${crypto.createHmac('sha1', salt).update('10.1.1.1').digest('base64')}`;
  const kh = write('known_hosts', [
    `192.168.8.236,spark1 ssh-ed25519 ${good.toString('base64')}`,
    `${hashed} ssh-ed25519 ${good.toString('base64')}`,
    `[10.2.2.2]:2222 ssh-ed25519 ${good.toString('base64')}`,
    `10.3.3.3 ssh-rsa ${rsa.toString('base64')}`,
    `@revoked * ssh-ed25519 ${other.toString('base64')}`, `spark1 ssh-ed25519 ${other.toString('base64')}`,
  ].join('\n'));
  assert.strictEqual(blobType(good), 'ssh-ed25519');
  assert.strictEqual(checkKnownHosts([kh], '192.168.8.236', 22, good), 'match');
  assert.strictEqual(checkKnownHosts([kh], '10.1.1.1', 22, good), 'match');
  assert.strictEqual(checkKnownHosts([kh], '10.2.2.2', 2222, good), 'match');
  assert.strictEqual(checkKnownHosts([kh], '10.2.2.2', 22, good), 'unknown');
  assert.strictEqual(checkKnownHosts([kh], '10.3.3.3', 22, good), 'unknown'); // different key type: not a conflict
  assert.strictEqual(checkKnownHosts([kh], '192.168.8.236', 22, blob('ssh-ed25519')), 'mismatch');
  assert.strictEqual(checkKnownHosts([kh], '192.168.8.236', 22, other), 'revoked');
  assert.strictEqual(checkKnownHosts([path.join(tmp, 'missing')], 'x', 22, good), 'unknown');
  assert.match(fingerprint(good), /^SHA256:[A-Za-z0-9+/]+$/);
});

test('ssh_config: Match host / all blocks apply, other criteria are skipped; ProxyJump becomes a jump chain', () => {
  const { matchApplies, parseJump } = require('../lib/sshconfig');
  const cfg = write('config-match', [
    'Host gpu*',
    '  ProxyJump ops@bastion.example.com:2222,inner',
    'Match host gpu-2',
    '  Port 2200',
    'Match user someone',
    '  Port 9999',
    'Match all',
    '  User everyone',
    'Host direct',
    '  ProxyJump none',
  ].join('\n'));
  const a = sshConfigFor('gpu-2', cfg);
  assert.strictEqual(a.port, 2200);
  assert.strictEqual(a.user, 'everyone');
  assert.deepStrictEqual(a.jumps, [{ user: 'ops', host: 'bastion.example.com', port: 2222 }, { user: null, host: 'inner', port: null }]);
  assert.deepStrictEqual(sshConfigFor('direct', cfg).jumps, []);
  assert.strictEqual(sshConfigFor('gpu-1', cfg).port, undefined);
  assert.strictEqual(matchApplies('host gpu-*,!gpu-9', 'gpu-9'), false);
  assert.strictEqual(matchApplies('!host other', 'gpu-9'), true);
  assert.strictEqual(matchApplies('exec "true"', 'gpu-9'), false);
  assert.deepStrictEqual(parseJump('[fd00::1]:22'), { user: null, host: 'fd00::1', port: 22 });
});
