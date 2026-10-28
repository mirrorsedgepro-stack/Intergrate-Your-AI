const test = require('node:test');
const assert = require('node:assert');
const { makeMessage, parseLine, makePresence, conflicts, MAX_TEXT } = require('../lib/collab');

test('messages are single-line JSON, capped and stripped of control characters', () => {
  const m = makeMessage({ from: { name: 'Alex', login: 'alex@x.com', client: 'c1' }, text: 'hi\u001b[2Jthere\nsecond line' + 'x'.repeat(5000) });
  const line = JSON.stringify(m);
  assert.ok(!line.includes('\n'), 'serialized message has no raw newline');
  assert.ok(Buffer.byteLength(line) < 4096, 'fits in one atomic append');
  assert.ok(!m.text.includes('\u001b'), 'escape sequences removed');
  assert.ok(m.text.includes('\n'), 'newlines inside the text are kept (escaped in JSON)');
  assert.ok(m.text.length <= MAX_TEXT);
  assert.strictEqual(m.kind, 'msg');
  assert.strictEqual(makeMessage({ from: {}, text: 'x', kind: 'evil' }).kind, 'msg');
});

test('parseLine round-trips and rejects junk', () => {
  const m = makeMessage({ from: { name: 'Sam' }, text: 'GPU free after 3pm', kind: 'status' });
  const back = parseLine(JSON.stringify(m));
  assert.strictEqual(back.id, m.id);
  assert.strictEqual(back.ts, m.ts);
  assert.strictEqual(back.text, 'GPU free after 3pm');
  assert.strictEqual(back.kind, 'status');
  assert.strictEqual(parseLine('not json'), null);
  assert.strictEqual(parseLine('{"id":1}'), null);
  assert.strictEqual(parseLine(''), null);
});

test('conflicts: same tmux session, same agent folder, ignores self and other accounts', () => {
  const sess = (o) => ({ host: 'spark1-aresj', user: 'jcee', preset: 'claude', ...o });
  const presence = [
    makePresence({ client: 'me', name: 'Me', sessions: [sess({ tmux: 'nx-claude-mine', cwd: '~/ARES-J' })] }),
    makePresence({ client: 'a', name: 'Alex', sessions: [sess({ tmux: 'nx-claude-1', cwd: '~/ARES-J/' }), sess({ tmux: 'nx-shell-2', preset: 'shell', cwd: '~/notes' })] }),
    makePresence({ client: 'b', name: 'Bea', sessions: [sess({ host: 'spark2-aresj', user: 'jcee-slave', tmux: 'nx-claude-1', cwd: '~/ARES-J' })] }),
  ];
  const s = conflicts(presence, 'me', { host: 'spark1-aresj', user: 'jcee', tmux: 'nx-claude-1' });
  assert.deepStrictEqual(s.sameSession.map((x) => x.person), ['Alex']);
  const f = conflicts(presence, 'me', { host: 'spark1-aresj', user: 'jcee', cwd: '~/ARES-J' });
  assert.deepStrictEqual(f.sameFolder.map((x) => x.person), ['Alex']);
  // a shell in the same folder is not an agent conflict
  assert.deepStrictEqual(conflicts(presence, 'me', { host: 'spark1-aresj', user: 'jcee', cwd: '~/notes' }).sameFolder, []);
  // my own sessions never conflict with me
  assert.deepStrictEqual(conflicts(presence, 'me', { host: 'spark1-aresj', user: 'jcee', tmux: 'nx-claude-mine' }).sameSession, []);
});

test('presence presets from others are reduced to plain ids', () => {
  const p = makePresence({ client: 'x', sessions: [{ preset: '__proto__' }, { preset: '<img src=x>' }, { preset: 'claude' }] });
  assert.deepStrictEqual(p.sessions.map((s) => s.preset), ['shell', 'shell', 'claude']);
});

test('claims: /claim parsing, presence carries them, expired or own claims are not conflicts', () => {
  const { parseClaim, claimsOn } = require('../renderer/util');
  const { makePresence } = require('../lib/collab');
  const now = new Date('2026-10-28T10:00:00').getTime();
  assert.deepStrictEqual(parseClaim('/claim gpu-box 1h30m fine-tuning', now), { host: 'gpu-box', until: now + 90 * 60000, note: 'fine-tuning' });
  assert.strictEqual(parseClaim('/claim gpu-box 15:00', now).until, new Date('2026-10-28T15:00:00').getTime());
  assert.strictEqual(parseClaim('/claim gpu-box 09:00', now).until, new Date('2026-10-29T09:00:00').getTime(), 'a past clock time means tomorrow');
  assert.strictEqual(parseClaim('/claim gpu-box soon', now), null);
  const p = makePresence({ client: 'a', name: 'Sam', claims: [{ host: 'gpu-box', until: now + 1000, note: 'x' }, { host: '', until: now + 1000 }] });
  assert.strictEqual(p.claims.length, 1);
  const presence = [p, makePresence({ client: 'b', name: 'Me', claims: [{ host: 'gpu-box', until: now + 1000 }] })];
  assert.deepStrictEqual(claimsOn(presence, 'gpu-box', 'b', now).map((c) => c.person), ['Sam']);
  assert.deepStrictEqual(claimsOn(presence, 'gpu-box', 'b', now + 2000), [], 'expired');
});
