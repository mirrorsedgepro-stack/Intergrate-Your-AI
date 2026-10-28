// Machine stats (GPU, CPU, RAM, disk, network) for the machine cards. The Linux script and its parser are shared by
// SSH machines (lib/remote), WSL distros and a local Linux box; local Windows/macOS have their own collectors in
// lib/local. Network throughput is a rate, so each machine keeps its previous counter sample here.

// One shot of everything the card shows. Each line is "key:value"; nvidia-smi is optional.
const LINUX_SCRIPT = 'nvidia-smi --query-gpu=name,utilization.gpu,temperature.gpu,memory.used,memory.total,power.draw --format=csv,noheader,nounits 2>/dev/null | head -n1 | sed "s/^/gpu:/"; '
  + 'echo "load:$(cut -d" " -f1 /proc/loadavg)"; echo "cpus:$(nproc)"; awk \'/^(MemTotal|MemAvailable):/{print "mem:"$1$2}\' /proc/meminfo; '
  // Disk: the filesystem $HOME lives on (where uploads, logs and most projects go), in 1K blocks.
  + 'df -Pk "$HOME" 2>/dev/null | awk \'NR==2{print "disk:"$2","$3","$4","$6}\'; '
  // Network: total bytes received/sent over every real interface (not lo, not docker/veth bridges), plus a clock.
  // (Interface names are right-aligned, so long ones have no leading blank: strip, then split on ": " runs.)
  + 'awk \'NR>2{sub(/^ +/,""); n=split($0,a,/[: ]+/); if (n>=10 && a[1]!~/^(lo|docker|veth|br-|virbr)/){rx+=a[2]; tx+=a[10]}} END{print "net:"rx+0","tx+0}\' /proc/net/dev 2>/dev/null; '
  + 'echo "now:$(date +%s%N 2>/dev/null || date +%s000000000)"';

const num = (v) => (v == null || /N\/A|^\s*$/.test(v) ? null : Number(v));

// stdout of LINUX_SCRIPT -> { gpu, load, cpus, memTotal, memAvail, disk, net } (net holds raw counters; see rate()).
function parseLinux(stdout) {
  const out = { gpu: null, load: null, cpus: null, memTotal: null, memAvail: null, disk: null, net: null };
  for (const line of String(stdout || '').split(/\r?\n/)) {
    let mm;
    if ((mm = line.match(/^gpu:(.*)$/))) {
      const f = mm[1].split(',').map((x) => x.trim());
      // nvidia-smi prints a one-line error (e.g. driver not loaded, no permission) instead of CSV.
      if (f.length < 6) { out.gpu = { error: mm[1].trim() }; continue; }
      const [name, util, temp, used, total, power] = f;
      out.gpu = { name, util: num(util), temp: num(temp), memUsed: num(used), memTotal: num(total), power: num(power) };
    } else if ((mm = line.match(/^load:([\d.]+)/))) out.load = Number(mm[1]);
    else if ((mm = line.match(/^cpus:(\d+)/))) out.cpus = Number(mm[1]);
    else if ((mm = line.match(/^mem:MemTotal:(\d+)/))) out.memTotal = Number(mm[1]) * 1024;
    else if ((mm = line.match(/^mem:MemAvailable:(\d+)/))) out.memAvail = Number(mm[1]) * 1024;
    else if ((mm = line.match(/^disk:(\d+),(\d+),(\d+),(.*)$/))) out.disk = { total: Number(mm[1]) * 1024, used: Number(mm[2]) * 1024, avail: Number(mm[3]) * 1024, mount: mm[4].trim() };
    else if ((mm = line.match(/^net:(\d+),(\d+)/))) out.net = { rx: Number(mm[1]), tx: Number(mm[2]) };
    else if ((mm = line.match(/^now:(\d+)/))) out.now = Number(mm[1]) / 1e6; // ms
  }
  return out;
}

// Turn two counter samples into bytes/second. prev may be null (first sample: no rate yet). Counters that went
// backwards (reboot, interface reset) give no rate either rather than a huge negative one.
function rate(prev, cur) {
  if (!cur || !cur.net) return null;
  const t = cur.now != null ? cur.now : Date.now();
  const sample = { rx: cur.net.rx, tx: cur.net.tx, t };
  if (!prev || prev.t == null || t <= prev.t || cur.net.rx < prev.rx || cur.net.tx < prev.tx) return { sample, rxBps: null, txBps: null };
  const dt = (t - prev.t) / 1000;
  return { sample, rxBps: (cur.net.rx - prev.rx) / dt, txBps: (cur.net.tx - prev.tx) / dt };
}

// Stateful helper: one per machine. finish(parsedStats) attaches rxBps/txBps and remembers the sample.
class NetRate {
  constructor() { this.prev = null; }
  finish(stats) {
    const r = rate(this.prev, stats);
    if (!r) return stats;
    this.prev = r.sample;
    stats.net = { ...stats.net, rxBps: r.rxBps, txBps: r.txBps };
    return stats;
  }
}

module.exports = { LINUX_SCRIPT, parseLinux, rate, NetRate };
