#!/usr/bin/env node
// Nexus MCP bridge: an MCP stdio server that forwards to the running Nexus app on this computer.
// Register it with an MCP client, e.g. Claude Code:  claude mcp add nexus -- node /path/to/scripts/nexus-mcp.js
// Nexus must be running with "Let MCP clients on this PC control Nexus" enabled in Settings; it writes the port and a
// per-run token to ~/.nexus/mcp.json, which this bridge reads.
const net = require('net');
const fs = require('fs');
const os = require('os');
const path = require('path');

const INFO = path.join(os.homedir(), '.nexus', 'mcp.json');
let info;
try { info = JSON.parse(fs.readFileSync(INFO, 'utf8')); } catch {
  process.stderr.write('nexus-mcp: Nexus is not running, or MCP is not enabled in its Settings (no ~/.nexus/mcp.json)\n');
  process.exit(1);
}

const sock = net.connect(info.port, '127.0.0.1');
sock.setEncoding('utf8');
sock.on('connect', () => sock.write(info.token + '\n'));
sock.on('error', (err) => { process.stderr.write(`nexus-mcp: cannot reach Nexus (${err.message})\n`); process.exit(1); });
sock.on('close', () => process.exit(0));

// stdio -> socket: MCP stdio is newline-delimited JSON, same as the socket; forward lines as they are.
let inBuf = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', (d) => {
  inBuf += d;
  let i;
  while ((i = inBuf.indexOf('\n')) >= 0) { const line = inBuf.slice(0, i); inBuf = inBuf.slice(i + 1); if (line.trim()) sock.write(line + '\n'); }
});
process.stdin.on('end', () => sock.end());
sock.on('data', (d) => process.stdout.write(d));
