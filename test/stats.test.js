const test = require('node:test');
const assert = require('node:assert');
const { parseLinux, rate, NetRate } = require('../lib/stats');

const SAMPLE = `gpu:NVIDIA GB10, 37, 61, 20480, 131072, 112.5
load:1.25
cpus:20
mem:MemTotal:131072000
mem:MemAvailable:65536000
disk:1000000,400000,550000,/home
net:1000000,500000
now:1700000000000000000
`;

test('parseLinux reads gpu, cpu, memory, disk and network counters', () => {
  const s = parseLinux(SAMPLE);
  assert.deepStrictEqual(s.gpu, { name: 'NVIDIA GB10', util: 37, temp: 61, memUsed: 20480, memTotal: 131072, power: 112.5 });
  assert.strictEqual(s.load, 1.25);
  assert.strictEqual(s.cpus, 20);
  assert.strictEqual(s.memTotal, 131072000 * 1024);
  assert.deepStrictEqual(s.disk, { total: 1000000 * 1024, used: 400000 * 1024, avail: 550000 * 1024, mount: '/home' });
  assert.deepStrictEqual(s.net, { rx: 1000000, tx: 500000 });
  assert.strictEqual(s.now, 1700000000000);
});

test('parseLinux keeps a missing GPU / disk / net as null and reports nvidia-smi errors', () => {
  const s = parseLinux('gpu:NVIDIA-SMI has failed because it couldn\'t communicate with the NVIDIA driver\nload:0.1\ncpus:4\n');
  assert.deepStrictEqual(s.gpu, { error: 'NVIDIA-SMI has failed because it couldn\'t communicate with the NVIDIA driver' });
  assert.strictEqual(s.disk, null);
  assert.strictEqual(s.net, null);
});

test('rate turns two counter samples into bytes per second and ignores resets', () => {
  const a = parseLinux('net:1000,2000\nnow:1000000000000\n');
  const b = parseLinux('net:6000,2000\nnow:1005000000000\n');
  const first = rate(null, a);
  assert.strictEqual(first.rxBps, null);
  const second = rate(first.sample, b);
  assert.strictEqual(second.rxBps, 1000);
  assert.strictEqual(second.txBps, 0);
  // Counter went backwards (reboot): no rate, but the new sample is kept.
  const reset = rate(second.sample, parseLinux('net:10,10\nnow:1010000000000\n'));
  assert.strictEqual(reset.rxBps, null);
  assert.strictEqual(reset.sample.rx, 10);
});

test('NetRate attaches throughput to successive stats', () => {
  const n = new NetRate();
  assert.strictEqual(n.finish(parseLinux('net:0,0\nnow:1000000000000\n')).net.rxBps, null);
  const s = n.finish(parseLinux('net:2048,1024\nnow:1002000000000\n'));
  assert.strictEqual(s.net.rxBps, 1024);
  assert.strictEqual(s.net.txBps, 512);
  assert.strictEqual(s.net.rx, 2048);
  assert.strictEqual(n.finish({ gpu: null, net: null }).net, null);
});
