//! Machine stats (GPU, CPU, RAM, disk, network) for the machine cards. The Linux script and its parser are shared by
//! SSH machines, WSL distros and a local Linux box; local Windows/macOS have their own collectors in `local`.
//! Network throughput is a rate, so each machine keeps its previous counter sample here.
use serde::Serialize;

/// One shot of everything the card shows. Each line is "key:value"; nvidia-smi is optional.
pub const LINUX_SCRIPT: &str = concat!(
    r#"nvidia-smi --query-gpu=name,utilization.gpu,temperature.gpu,memory.used,memory.total,power.draw --format=csv,noheader,nounits 2>/dev/null | head -n1 | sed "s/^/gpu:/"; "#,
    r#"echo "load:$(cut -d" " -f1 /proc/loadavg)"; echo "cpus:$(nproc)"; awk '/^(MemTotal|MemAvailable):/{print "mem:"$1$2}' /proc/meminfo; "#,
    // Disk: the filesystem $HOME lives on (where uploads, logs and most projects go), in 1K blocks.
    r#"df -Pk "$HOME" 2>/dev/null | awk 'NR==2{print "disk:"$2","$3","$4","$6}'; "#,
    // Network: total bytes received/sent over every real interface (not lo, not docker/veth bridges), plus a clock.
    r#"awk 'NR>2{sub(/^ +/,""); n=split($0,a,/[: ]+/); if (n>=10 && a[1]!~/^(lo|docker|veth|br-|virbr)/){rx+=a[2]; tx+=a[10]}} END{print "net:"rx+0","tx+0}' /proc/net/dev 2>/dev/null; "#,
    r#"echo "now:$(date +%s%N 2>/dev/null || date +%s000000000)""#
);

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Gpu {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub name: String,
    pub util: Option<f64>,
    pub temp: Option<f64>,
    pub mem_used: Option<f64>,
    pub mem_total: Option<f64>,
    pub power: Option<f64>,
}
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Disk { pub total: f64, pub used: f64, pub avail: f64, pub mount: String }
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Net { pub rx: f64, pub tx: f64, pub rx_bps: Option<f64>, pub tx_bps: Option<f64> }
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub gpu: Option<Gpu>,
    pub load: Option<f64>,
    pub cpus: Option<f64>,
    pub mem_total: Option<f64>,
    pub mem_avail: Option<f64>,
    pub disk: Option<Disk>,
    pub net: Option<Net>,
    /// Sample time in ms (the machine's own clock).
    pub now: Option<f64>,
}

fn num(v: &str) -> Option<f64> {
    if v.contains("N/A") || v.trim().is_empty() { None } else { v.trim().parse().ok() }
}
fn lead_num(s: &str) -> Option<f64> {
    let end = s.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(s.len());
    s[..end].parse().ok()
}

pub fn parse_gpu_csv(csv: &str) -> Gpu {
    let f: Vec<&str> = csv.split(',').map(str::trim).collect();
    // nvidia-smi prints a one-line error (e.g. driver not loaded, no permission) instead of CSV.
    if f.len() < 6 { return Gpu { error: Some(csv.trim().to_string()), ..Default::default() }; }
    Gpu { error: None, name: f[0].into(), util: num(f[1]), temp: num(f[2]), mem_used: num(f[3]), mem_total: num(f[4]), power: num(f[5]) }
}

/// stdout of LINUX_SCRIPT -> Stats (net holds raw counters; see `rate`).
pub fn parse_linux(stdout: &str) -> Stats {
    let mut out = Stats::default();
    for line in stdout.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(v) = line.strip_prefix("gpu:") { out.gpu = Some(parse_gpu_csv(v)); }
        else if let Some(v) = line.strip_prefix("load:") { if let Some(n) = lead_num(v) { out.load = Some(n); } }
        else if let Some(v) = line.strip_prefix("cpus:") { if let Some(n) = lead_num(v) { out.cpus = Some(n); } }
        else if let Some(v) = line.strip_prefix("mem:MemTotal:") { out.mem_total = lead_num(v).map(|n| n * 1024.0); }
        else if let Some(v) = line.strip_prefix("mem:MemAvailable:") { out.mem_avail = lead_num(v).map(|n| n * 1024.0); }
        else if let Some(v) = line.strip_prefix("disk:") {
            let p: Vec<&str> = v.splitn(4, ',').collect();
            if let (4, Some(t), Some(u), Some(a)) = (p.len(), p.first().and_then(|x| x.parse::<f64>().ok()), p.get(1).and_then(|x| x.parse::<f64>().ok()), p.get(2).and_then(|x| x.parse::<f64>().ok())) {
                out.disk = Some(Disk { total: t * 1024.0, used: u * 1024.0, avail: a * 1024.0, mount: p[3].trim().into() });
            }
        } else if let Some(v) = line.strip_prefix("net:") {
            if let Some((r, t)) = v.split_once(',') {
                if let (Some(rx), Some(tx)) = (lead_num(r), lead_num(t)) { out.net = Some(Net { rx, tx, ..Default::default() }); }
            }
        } else if let Some(v) = line.strip_prefix("now:") { out.now = lead_num(v).map(|n| n / 1e6); }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample { pub rx: f64, pub tx: f64, pub t: f64 }

/// Turn two counter samples into bytes/second. `prev` may be None (first sample: no rate yet). Counters that went
/// backwards (reboot, interface reset) give no rate either rather than a huge negative one.
pub fn rate(prev: Option<Sample>, cur: &Stats, fallback_now: f64) -> Option<(Sample, Option<f64>, Option<f64>)> {
    let net = cur.net.as_ref()?;
    let t = cur.now.unwrap_or(fallback_now);
    let sample = Sample { rx: net.rx, tx: net.tx, t };
    match prev {
        Some(p) if t > p.t && net.rx >= p.rx && net.tx >= p.tx => {
            let dt = (t - p.t) / 1000.0;
            Some((sample, Some((net.rx - p.rx) / dt), Some((net.tx - p.tx) / dt)))
        }
        _ => Some((sample, None, None)),
    }
}

/// Stateful helper: one per machine. `finish` attaches rxBps/txBps and remembers the sample.
#[derive(Default)]
pub struct NetRate { prev: Option<Sample> }
impl NetRate {
    pub fn finish(&mut self, mut stats: Stats) -> Stats {
        if let Some((sample, rx, tx)) = rate(self.prev, &stats, crate::util::now_ms() as f64) {
            self.prev = Some(sample);
            if let Some(n) = stats.net.as_mut() { n.rx_bps = rx; n.tx_bps = tx; }
        }
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const OUT: &str = "gpu:NVIDIA GB10, 37, 52, 1024, 8192, 41.50\nload:1.25\ncpus:20\nmem:MemTotal:131072000\nmem:MemAvailable:65536000\ndisk:1000000,400000,600000,/home\nnet:123456,654321\nnow:1700000000000000000\n";

    #[test]
    fn parse_linux_reads_everything() {
        let s = parse_linux(OUT);
        let g = s.gpu.unwrap();
        assert_eq!((g.name.as_str(), g.util, g.temp, g.power), ("NVIDIA GB10", Some(37.0), Some(52.0), Some(41.5)));
        assert_eq!((s.load, s.cpus), (Some(1.25), Some(20.0)));
        assert_eq!(s.mem_total, Some(131072000.0 * 1024.0));
        assert_eq!(s.disk.unwrap(), Disk { total: 1e6 * 1024.0, used: 4e5 * 1024.0, avail: 6e5 * 1024.0, mount: "/home".into() });
        assert_eq!(s.net.unwrap().rx, 123456.0);
        assert_eq!(s.now, Some(1.7e12));
    }
    #[test]
    fn missing_parts_stay_none_and_gpu_errors_are_reported() {
        let s = parse_linux("load:0.10\ncpus:4\n");
        assert!(s.gpu.is_none() && s.disk.is_none() && s.net.is_none());
        let g = parse_linux("gpu:NVIDIA-SMI has failed because it couldn't communicate with the NVIDIA driver.\n").gpu.unwrap();
        assert!(g.error.unwrap().contains("NVIDIA-SMI has failed"));
        let g = parse_linux("gpu:GB10, [N/A], 40, [N/A], [N/A], [N/A]\n").gpu.unwrap();
        assert_eq!((g.util, g.temp), (None, Some(40.0)));
    }
    #[test]
    fn rate_and_resets() {
        let st = |rx, tx, now| Stats { net: Some(Net { rx, tx, ..Default::default() }), now: Some(now), ..Default::default() };
        let (s1, a, b) = rate(None, &st(1000.0, 500.0, 10_000.0), 0.0).unwrap();
        assert_eq!((a, b), (None, None));
        let (_, a, b) = rate(Some(s1), &st(6000.0, 1500.0, 15_000.0), 0.0).unwrap();
        assert_eq!((a, b), (Some(1000.0), Some(200.0)));
        let (_, a, _) = rate(Some(s1), &st(10.0, 1500.0, 15_000.0), 0.0).unwrap();
        assert_eq!(a, None);
        assert!(rate(None, &Stats::default(), 0.0).is_none());
    }
    #[test]
    fn net_rate_is_stateful() {
        let mut r = NetRate::default();
        let st = |rx, now| Stats { net: Some(Net { rx, tx: 0.0, ..Default::default() }), now: Some(now), ..Default::default() };
        assert_eq!(r.finish(st(0.0, 1000.0)).net.unwrap().rx_bps, None);
        assert_eq!(r.finish(st(5000.0, 6000.0)).net.unwrap().rx_bps, Some(1000.0));
    }
}
