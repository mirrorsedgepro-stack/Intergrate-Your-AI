const test = require('node:test');
const assert = require('node:assert');
const { fuzzy, mentions, isExeTitle } = require('../renderer/util');

test('palette fuzzy: words match in any order, letters within a word in order', () => {
  const item = 'Shell in folder… on Spark 1';
  assert.ok(fuzzy('in folder shell spark 1', item), 'Ctrl+Shift+N prefix followed by more words');
  assert.ok(fuzzy('spark 1 shell', item));
  assert.ok(fuzzy('shl', item));
  assert.strictEqual(fuzzy('lhs', 'Shell'), null, 'letters out of order within a word');
  assert.strictEqual(fuzzy('claude', item), null);
  assert.ok(fuzzy('', item), 'empty query matches everything');
});

test('palette fuzzy: tighter and word-start matches rank higher', () => {
  const a = fuzzy('spark', 'New Shell on Spark 2');
  const b = fuzzy('spark', 'Settings: please park sessions');
  assert.ok(a.score > b.score);
  // Highlights point at the best occurrence ("Spark", not the scattered s…p…a…r…k).
  const text = 'settings spark';
  const hits = [...fuzzy('spark', text).hits].sort((x, y) => x - y);
  assert.deepStrictEqual(hits, [9, 10, 11, 12, 13]);
});

test('@mentions: first name, case-insensitive, whole word', () => {
  assert.ok(mentions('@jed can you look?', 'Jed Smith'));
  assert.ok(mentions('thanks @JED', 'Jed'));
  assert.ok(mentions('(@jed) done', 'jed'));
  assert.ok(!mentions('@jedi council', 'Jed'));
  assert.ok(!mentions('email jed@example.com', 'Jed'));
  assert.ok(!mentions('no mention here', 'Jed'));
  assert.ok(!mentions('@a.b', ''), 'no name, never mentioned');
  assert.ok(mentions('hi @a.b', 'a.b'), 'regex characters in names are literal');
  assert.ok(!mentions('hi @axb', 'a.b'));
});

test('Windows shell exe-path titles are recognised', () => {
  assert.ok(isExeTitle('C:\\Program Files\\PowerShell\\7\\pwsh.exe'));
  assert.ok(isExeTitle('Administrator: C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe'));
  assert.ok(isExeTitle('cmd.exe'));
  assert.ok(!isExeTitle('jcee@spark1: ~/ARES-J'));
  assert.ok(!isExeTitle('Claude Code'));
});
