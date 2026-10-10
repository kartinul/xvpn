#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Os {
    MacOs,
    Linux,
}

pub fn os() -> Os {
    if cfg!(target_os = "macos") {
        Os::MacOs
    } else if cfg!(target_os = "linux") {
        Os::Linux
    } else {
        panic!("xvpn supports macOS and Linux")
    }
}

pub fn os_name() -> &'static str {
    match os() {
        Os::MacOs => "macOS",
        Os::Linux => "Linux",
    }
}

pub fn supports_gateway() -> bool {
    os() == Os::Linux
}

pub fn router() -> Option<String> {
    imp::router()
}

pub fn active_device() -> Option<String> {
    imp::active_device()
}

pub fn dns_servers(dev: &str) -> Vec<String> {
    imp::dns_servers(dev)
}

#[cfg(target_os = "macos")]
mod imp {
    fn capture(prog: &str, args: &[&str]) -> Option<String> {
        use crate::vpn::config::output;
        let out = output(prog, args).ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn wifi_device() -> Option<String> {
        let out = capture("networksetup", &["-listallhardwareports"])?;
        let mut lines = out.lines();
        while let Some(line) = lines.next() {
            if line.contains("Wi-Fi") || line.contains("AirPort") {
                let dev = lines
                    .next()?
                    .split_once(':')
                    .map(|(_, dev)| dev.trim())
                    .unwrap_or_default();
                if !dev.is_empty() {
                    return Some(dev.to_string());
                }
            }
        }
        None
    }

    pub fn dns_servers(dev: &str) -> Vec<String> {
        let Some(out) = capture("ipconfig", &["getoption", dev, "domain_name_server"]) else {
            return Vec::new();
        };
        out.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && super::prefix4(l).is_some())
            .map(str::to_string)
            .collect()
    }

    pub fn active_device() -> Option<String> {
        if let Some(out) = capture("route", &["-n", "get", "default"]) {
            for line in out.lines() {
                if let Some(iface) = line.trim().strip_prefix("interface:") {
                    let iface = iface.trim();
                    if !iface.is_empty() && !iface.starts_with("utun") {
                        return Some(iface.to_string());
                    }
                }
            }
        }
        wifi_device()
    }

    pub fn router() -> Option<String> {
        let dev = active_device()?;
        capture("ipconfig", &["getoption", &dev, "router"])
            .map(|out| out.trim().to_string())
            .filter(|s| super::prefix4(s).is_some())
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::parse_proc_route;
    use std::fs;

    fn wifi_devices() -> Vec<String> {
        let Ok(entries) = fs::read_dir("/sys/class/net") else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|e| e.path().join("wireless").is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect()
    }

    pub fn active_device() -> Option<String> {
        wifi_devices().into_iter().next()
    }

    pub fn dns_servers(_dev: &str) -> Vec<String> {
        let Ok(text) = fs::read_to_string("/etc/resolv.conf") else {
            return Vec::new();
        };
        text.lines()
            .filter_map(|l| l.trim().strip_prefix("nameserver "))
            .map(str::trim)
            .filter(|addr| !super::is_loopback(addr) && super::prefix4(addr).is_some())
            .map(str::to_string)
            .collect()
    }

    pub fn router() -> Option<String> {
        parse_proc_route(&fs::read_to_string("/proc/net/route").ok()?)
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_proc_route(text: &str) -> Option<String> {
    text.lines().skip(1).find_map(|line| {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 3 || cols[1] != "00000000" || cols[2].len() != 8 {
            return None;
        }
        let bytes: Option<Vec<u8>> = cols[2]
            .as_bytes()
            .chunks(2)
            .rev()
            .map(|c| u8::from_str_radix(std::str::from_utf8(c).ok()?, 16).ok())
            .collect();
        let b = bytes?;
        Some(format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]))
    })
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn is_loopback(addr: &str) -> bool {
    matches!(prefix4(addr), Some([127, _, _]))
}

pub(crate) fn prefix4(addr: &str) -> Option<[u8; 3]> {
    let parts: Vec<&str> = addr.trim().split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let octets: Vec<u8> = parts
        .iter()
        .map(|p| p.parse::<u8>().ok())
        .collect::<Option<Vec<u8>>>()?;
    Some([octets[0], octets[1], octets[2]])
}
