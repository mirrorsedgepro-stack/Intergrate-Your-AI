const test = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawn } = require('child_process');
const { McpServer, plain, INFO_FILE } = require('../lib/mcp');

test('plain strips terminal escapes', () => {
  assert.strictEqual(plain('\x1b[32mok\x1b[0m\r\n\x1b]0;title\x07done'), 'ok\ndone');
});

test('MCP server: token-gated JSON-RPC over loopback, tools work through the stdio bridge', async () => {
  const sent = [];
  const srv = new McpServer({
    version: '9.9.9',
    listMachines: async () => [{ id: 'local', name: 'This PC', status: 'online' }],
    listSessions: async () => [{ id: 's1', preset: 'claude' }],
    launch: async ({ machine, preset, cwd }) => ({ id: 's2', machine, preset, cwd }),
    send: async (id, text) => { sent.push([id, text]); },
    readOutput: async () => '\x1b[1mhello\x1b[0m\r\nworld',
    close: async () => {},
  });
  await srv.start();
  const info = JSON.parse(fs.readFileSync(INFO_FILE, 'utf8'));
  assert.strictEqual(info.port, srv.port);
  if (process.platform !== 'win32') assert.strictEqual(fs.statSync(INFO_FILE).mode & 0o777, 0o600);

  const bridge = spawn(process.execPath, [path.join(__dirname, '..', 'scripts', 'nexus-mcp.js')], { stdio: ['pipe', 'pipe', 'pipe'] });
  const replies = [];
  let buf = '';
  const got = (n) => new Promise((resolve) => { const tick = () => (replies.length >= n ? resolve() : setTimeout(tick, 20)); tick(); });
  bridge.stdout.on('data', (d) => { buf += d; let i; while ((i = buf.indexOf('\n')) >= 0) { replies.push(JSON.parse(buf.slice(0, i))); buf = buf.slice(i + 1); } });
  const call = (id, method, params) => bridge.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
  call(1, 'initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 't', version: '0' } });
  bridge.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
  call(2, 'tools/list');
  call(3, 'tools/call', { name: 'launch', arguments: { machine: 'local', preset: 'claude', cwd: '~/x' } });
  call(4, 'tools/call', { name: 'send', arguments: { session: 's2', text: 'hi\n' } });
  call(5, 'tools/call', { name: 'read_output', arguments: { session: 's2' } });
  call(6, 'tools/call', { name: 'nope', arguments: {} });
  await got(6);
  assert.strictEqual(replies[0].result.serverInfo.version, '9.9.9');
  assert.ok(replies[1].result.tools.some((t) => t.name === 'launch'));
  assert.deepStrictEqual(JSON.parse(replies[2].result.content[0].text), { id: 's2', machine: 'local', preset: 'claude', cwd: '~/x' });
  assert.deepStrictEqual(sent, [['s2', 'hi\n']]);
  assert.strictEqual(replies[4].result.content[0].text, 'hello\nworld');
  assert.strictEqual(replies[5].error.code, -32602);
  bridge.kill();

  // Wrong token: dropped before any method runs.
  const net = require('net');
  const dropped = await new Promise((resolve) => {
    const s = net.connect(srv.port, '127.0.0.1', () => s.write('bad-token\n' + JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'tools/list' }) + '\n'));
    let any = false;
    s.on('data', () => { any = true; });
    s.on('close', () => resolve(!any));
    s.on('error', () => resolve(true));
  });
  assert.strictEqual(dropped, true);
  srv.stop();
  assert.ok(!fs.existsSync(INFO_FILE));
});
