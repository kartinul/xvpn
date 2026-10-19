use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::cli::Mode;
use crate::platform::network;
use crate::utils::output as out;
use crate::utils::read_json;
use crate::vpn::config::{self};
use crate::vpn::profile;
use crate::vpn::supervisor;
use crate::Result;

pub fn set_mode(root: &Path, mode: Mode) -> Result<()> {
    let mode = mode.as_str();
    supervisor::authorize_transition(root, mode)?;
    let path = root.join(config::MODE);
    fs::write(&path, mode).map_err(|e| format!("writing mode: {e}"))?;
    let written = std::time::SystemTime::now();
    out::kv("mode", mode);

    if !supervisor::is_running() {
        out::warn("no supervisor is running, so nothing was applied");
        out::info("start one with `make agent` (auto-start) or `xvpn supervise`");
        return Ok(());
    }

    match wait_for_state(written, Duration::from_secs(10)) {
        Some(state) => match state.as_str() {
            "on" | "selective" => out::kv("active", &state),
            "failed" => {
                return Err(
                    "supervisor rejected the config and stayed down — see `xvpn logs`".into(),
                )
            }
            other => out::kv("active", &format!("{other} (nothing is routing)")),
        },
        None => {
            out::warn("no state update received from supervisor");
        }
    }
    Ok(())
}

fn wait_for_state(changed_at: std::time::SystemTime, timeout: Duration) -> Option<String> {
    let path = config::state_file();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        let fresh = meta.modified().map(|m| m >= changed_at).unwrap_or(false);
        if !fresh {
            continue;
        }
        let state = fs::read_to_string(&path).ok()?.trim().to_string();
        if !state.is_empty() {
            return Some(state);
        }
    }
    None
}

pub fn status(root: &Path) -> Result<()> {
    let mode = read_mode(root);
    let active = fs::read_to_string(config::state_file())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "stopped".to_string());

    out::kv("mode", &mode);
    out::kv("active", &active);
    out::kv("root", &root.display().to_string());

    if let Some(notice) = config::stale_root_notice() {
        out::warn(&notice);
    }

    match profile::active_name(root) {
        Some(name) => {
            let server = profile::list(root)
                .into_iter()
                .find(|p| p.name == name)
                .and_then(|p| p.server)
                .unwrap_or_else(|| "(unreadable)".into());
            out::kv("profile", &format!("{name} ({server})"));
        }
        None => out::kv("profile", "none (run `xvpn import`)"),
    }

    if mode == "default" {
        print_routing(root);
    }
    Ok(())
}

pub fn read_mode(root: &Path) -> String {
    fs::read_to_string(root.join(config::MODE))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unset".to_string())
}

fn print_routing(root: &Path) {
    let path = root.join(config::DEFAULT);
    let cfg = match read_json(&path) {
        Ok(c) => c,
        Err(_) => {
            out::kv("routed", "unknown (run `xvpn repair`)");
            return;
        }
    };
    let apps = network::app_names(&cfg);
    let sites = network::site_names(&cfg);

    if !apps.is_empty() {
        out::section("apps:");
        out::list(&apps.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    }
    if !sites.is_empty() {
        out::section("sites (and subdomains):");
        out::list(&sites.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    }
    if apps.is_empty() && sites.is_empty() {
        out::status("→", "nothing else routed, everything goes direct");
    } else {
        out::status("→", "everything else direct");
    }
}
