use std::path::Path;
use std::process::{Command, Stdio};

use crate::utils::output as out;
use crate::vpn::config::{self};
use crate::vpn::profile;
use crate::Result;

pub fn check_vpn(root: &Path) -> Result<()> {
    let (name, _) = profile::resolve_active(root)?;
    out::kv("checking profile", &name);

    for (scheme, port) in PROBES {
        let proxy = format!("{scheme}://127.0.0.1:{port}");
        match run_curl(&proxy) {
            Ok(ip) => {
                out::kv("via proxy", &format!("{proxy} → {ip}"));
                return Ok(());
            }
            Err(e) => out::kv(&proxy, &e),
        }
    }
    Err("no running proxy found — start the VPN first".into())
}

fn run_curl(proxy: &str) -> Result<String> {
    let out = Command::new("curl")
        .args([
            "-4",
            "--retry",
            "3",
            "--retry-all-errors",
            "-s",
            "-x",
            proxy,
            IFCONFIG_CHECK_HOST,
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("curl failed: {e}"))?;
    if out.status.success() {
        let ip = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if ip.is_empty() {
            return Err("empty response".into());
        }
        Ok(ip)
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() {
            format!("curl exited with {}", out.status)
        } else {
            err
        })
    }
}

pub(crate) const PROBES: [(&str, u16); 2] =
    [("socks5", config::SOCKS_PORT), ("http", config::HTTP_PORT)];
pub(crate) const IP_CHECK_HOST: &str = "ip.me";
pub(crate) const IFCONFIG_CHECK_HOST: &str = "ifconfig.me";

pub(crate) const IPV4_FALLBACK_IP_URL: &str = "https://api4.ipify.org";
pub(crate) const IPV4_FALLBACK_IFCONFIG_URL: &str = "https://ipv4.icanhazip.com";
