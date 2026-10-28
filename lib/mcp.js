// Nexus as an MCP server, so an agent running on this computer (Claude Code in a Nexus pane, or any MCP client) can
// orchestrate the others: list machines and sessions, launch an agent on a machine, type into a session, read what
// it printed. Opt-in (Settings). Transport: newline-delimited JSON-RPC on a loopback TCP port with a per-run token
// written to ~/.nexus/mcp.json (mode 600) - only processes running as this user can read it. scripts/nexus-mcp.js
// is the stdio bridge MCP clients spawn: `claude mcp add nexus -- node <path>/scripts/nexus-mcp.js`.
const net = require('net');
const fs = require('fs');
const os = require('os');
const path = require('path');
const crypto = require('crypto');

const PROTOCOL = '2024-11-05';
const INFO_FILE = path.join(os.homedir(), '.nexus', 'mcp.json');

// Terminal output -> plain text (CSI/OSC sequences and carriage returns stripped) for read_output.
function plain(s) {
  return String(s || '')
    .replace(/\x1b\][^\x07\x1b]*(\x07|\x1b\\)/g, '')   // OSC
    .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '')            // CSI
    .replace(/\x1b[()][A-Za-z0-9]/g, '')                // charset
    .replace(/\x1b[=>]/g, '')
    .replace(/\r\n?/g, '\n')
    .replace(/[\x00-\x08\x0b-\x1f\x7f]/g, '');
}

const TOOLS = [
  { name: 'list_machines', description: 'The machines Nexus knows (this PC, WSL distros, SSH machines) with connection status and the latest GPU/CPU/RAM/disk/network stats.', inputSchema: { type: 'object', properties: {} } },
  { name: 'list_sessions', description: 'Open terminal sessions: id, machine, preset (claude, agy, openclaw, shell), tmux name, and the last line printed.', inputSchema: { type: 'object', properties: {} } },
  { name: 'launch', description: 'Start a session on a machine, optionally in a folder. Returns the new session id. presets: claude, antigravity, openclaw, shell, or any preset id from Nexus.', inputSchema: { type: 'object', properties: { machine: { type: 'string', description: 'machine id or name' }, preset: { type: 'string', default: 'shell' }, cwd: { type: 'string', description: 'folder on that machine, e.g. ~/proj' } }, required: ['machine'] } },
  { name: 'send', description: 'Type text into a session (append "\\n" to submit). Use for prompts to another agent or shell commands.', inputSchema: { type: 'object', properties: { session: { type: 'string' }, text: { type: 'string' } }, required: ['session', 'text'] } },
  { name: 'read_output', description: 'The most recent output of a session as plain text (up to 64 KB).', inputSchema: { type: 'object', properties: { session: { type: 'string' }, max_chars: { type: 'integer', default: 8000 } }, required: ['session'] } },
  { name: 'close', description: 'Close a session (remote tmux sessions keep running unless kill is true).', inputSchema: { type: 'object', properties: { session: { type: 'string' }, kill: { type: 'boolean', default: false } }, required: ['session'] } },
];

class McpServer {
  // handlers: { listMachines(), listSessions(), launch({machine,preset,cwd}), send(id,text), readOutput(id,max), close(id,kill) }
  constructor(handlers) {
    this.h = handlers;
    this.server = null;
    this.token = null;
    this.port = null;
  }

  start() {
    return new Promise((resolve, reject) => {
      this.token = crypto.randomBytes(24).toString('base64url');
      const server = net.createServer((sock) => this.onClient(sock));
      server.on('error', reject);
      server.listen(0, '127.0.0.1', () => {
        this.server = server;
        this.port = server.address().port;
        try {
          fs.mkdirSync(path.dirname(INFO_FILE), { recursive: true, mode: 0o700 });
          fs.writeFileSync(INFO_FILE, JSON.stringify({ port: this.port, token: this.token, pid: process.pid }), { mode: 0o600 });
        } catch (err) { server.close(); return reject(err); }
        resolve(this);
      });
    });
  }

  stop() {
    if (this.server) { this.server.close(); this.server = null; }
    try { const cur = JSON.parse(fs.readFileSync(INFO_FILE, 'utf8')); if (cur.pid === process.pid) fs.unlinkSync(INFO_FILE); } catch { /* not ours / gone */ }
  }

  onClient(sock) {
    let buf = '', authed = false;
    const reply = (obj) => { try { sock.write(JSON.stringify(obj) + '\n'); } catch { /* client gone */ } };
    sock.setEncoding('utf8');
    sock.on('error', () => {});
    sock.on('data', async (d) => {
      buf += d;
      if (buf.length > 4 * 1024 * 1024) { sock.destroy(); return; }
      let i;
      while ((i = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, i).trim();
        buf = buf.slice(i + 1);
        if (!line) continue;
        if (!authed) { authed = line === this.token; if (!authed) { sock.destroy(); return; } continue; }
        let msg;
        try { msg = JSON.parse(line); } catch { reply({ jsonrpc: '2.0', id: null, error: { code: -32700, message: 'Parse error' } }); continue; }
        if (msg.id === undefined) continue; // notification
        try { reply({ jsonrpc: '2.0', id: msg.id, result: await this.handle(msg) }); }
        catch (err) { reply({ jsonrpc: '2.0', id: msg.id, error: { code: err.code || -32603, message: err.message } }); }
      }
    });
  }

  async handle(msg) {
    const p = msg.params || {};
    switch (msg.method) {
      case 'initialize': return { protocolVersion: PROTOCOL, capabilities: { tools: {} }, serverInfo: { name: 'nexus', version: this.h.version || '0' } };
      case 'ping': return {};
      case 'tools/list': return { tools: TOOLS };
      case 'tools/call': return this.call(p.name, p.arguments || {});
      default: { const e = new Error(`Method not found: ${msg.method}`); e.code = -32601; throw e; }
    }
  }

  async call(name, a) {
    const text = (v) => ({ content: [{ type: 'text', text: typeof v === 'string' ? v : JSON.stringify(v, null, 2) }] });
    const str = (v, n = 4000) => String(v == null ? '' : v).slice(0, n);
    switch (name) {
      case 'list_machines': return text(await this.h.listMachines());
      case 'list_sessions': return text(await this.h.listSessions());
      case 'launch': return text(await this.h.launch({ machine: str(a.machine, 100), preset: str(a.preset || 'shell', 24), cwd: a.cwd ? str(a.cwd, 300) : undefined }));
      case 'send': await this.h.send(str(a.session, 32), str(a.text, 64 * 1024)); return text('ok');
      case 'read_output': return text(plain(await this.h.readOutput(str(a.session, 32))).slice(-Math.min(Math.max(Number(a.max_chars) || 8000, 100), 65536)));
      case 'close': await this.h.close(str(a.session, 32), !!a.kill); return text('ok');
      default: { const e = new Error(`Unknown tool: ${name}`); e.code = -32602; throw e; }
    }
  }
}

module.exports = { McpServer, plain, TOOLS, INFO_FILE, PROTOCOL };
