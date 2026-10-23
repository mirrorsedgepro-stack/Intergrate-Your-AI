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
