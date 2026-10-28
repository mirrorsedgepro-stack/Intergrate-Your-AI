// Local port forwarding over a machine's existing SSH connection (ssh -L): a remote web UI (OpenClaw gateway, Jupyter,
// TensorBoard, a dev server) becomes http://localhost:<port> on this computer. One listener per tunnel, bound to
// loopback only, so nothing else on the LAN can ride it.
const net = require('net');

class Tunnel {
  // machine: a lib/remote Machine (connected). remoteHost/remotePort: where the remote service listens, as seen from
  // that machine. localPort 0 picks a free port.
  constructor(machine, { remoteHost = '127.0.0.1', remotePort, localPort = 0 }) {
    this.machine = machine;
    this.remoteHost = remoteHost;
    this.remotePort = Number(remotePort);
    this.localPort = Number(localPort) || 0;
    this.server = null;
    this.sockets = new Set();
    this.bytes = 0;
  }

  start() {
    return new Promise((resolve, reject) => {
      if (!(this.remotePort > 0 && this.remotePort < 65536)) return reject(new Error('Remote port must be 1-65535'));
      const server = net.createServer((sock) => {
        const conn = this.machine.conn;
        if (!conn) { sock.destroy(); return; }
        this.sockets.add(sock);
        sock.on('close', () => this.sockets.delete(sock));
        conn.forwardOut('127.0.0.1', sock.localPort, this.remoteHost, this.remotePort, (err, stream) => {
          if (err) { sock.destroy(); return; }
          stream.on('data', (d) => { this.bytes += d.length; });
          sock.on('error', () => stream.destroy());
          stream.on('error', () => sock.destroy());
          sock.pipe(stream).pipe(sock);
        });
      });
      server.on('error', (err) => reject(err.code === 'EADDRINUSE' ? new Error(`Port ${this.localPort} is already in use on this computer`) : err));
      server.listen(this.localPort, '127.0.0.1', () => {
        this.server = server;
        this.localPort = server.address().port;
        resolve(this);
      });
    });
  }

  stop() {
    for (const s of this.sockets) s.destroy();
    this.sockets.clear();
    if (this.server) { this.server.close(); this.server = null; }
  }

  info() {
    return { localPort: this.localPort, remoteHost: this.remoteHost, remotePort: this.remotePort, connections: this.sockets.size, bytes: this.bytes };
  }
}

module.exports = { Tunnel };
