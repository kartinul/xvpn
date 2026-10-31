use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::utils::output as out;
use crate::utils::{read_json, write_json};
use crate::vpn::config::{self};
use crate::vpn::profile;
use crate::Result;

pub fn ensure_dependencies() -> Result<()> {
    for tool in ["xray", "sing-box"] {
        if find_dependency(tool).is_none() {
            return Err(format!(
                "{tool} not found. Install {tool} and put it in PATH, then retry"
            ));
        }
    }
    Ok(())
}

fn find_dependency(tool: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(tool))
        .find(|candidate| candidate.is_file())
}

pub fn check(root: &Path) -> Result<()> {
    if let Err(e) = ensure_dependencies() {
        out::warn(&e);
        out::info("run ./install.sh to install missing dependencies");
        return Ok(());
    }
    let mut ok = true;

    match profile::resolve_active(root) {
        Ok((name, path)) => match config::check_xray(&path) {
            Ok(()) => out::status("✓", &format!("profile '{name}': OK")),
            Err(e) => {
                out::status("✗", &format!("profile '{name}': FAILED"));
                out::error(&e);
                ok = false;
            }
        },
        Err(e) => out::error(&e),
    }

    for (name, _) in sidecars() {
        let path = root.join(name);
        if !path.exists() {
            out::status("✗", &format!("{name}: MISSING (run `xvpn repair`)"));
            ok = false;
            continue;
        }
        match config::check_singbox(&path) {
            Ok(()) => out::status("✓", &format!("{name}: OK")),
            Err(e) => {
                out::status("✗", &format!("{name}: FAILED"));
                out::error(&e);
                ok = false;
            }
        }
    }

    if !ok {
        return Err(String::new());
    }
    Ok(())
}

pub fn sidecars() -> [(&'static str, config::Scope); 2] {
    [
        (config::ON, config::Scope::Global),
        (config::DEFAULT, config::Scope::Selective),
    ]
}

// the current network's rules, created empty on first use
pub fn network_config(root: &Path) -> Result<PathBuf> {
    let path = crate::platform::network::network_rules(root)?;
    if !path.is_file() {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        write_json(&path, &config::singbox_config(config::Scope::Selective))?;
    }
    Ok(path)
}

pub fn refresh_sidecars(root: &Path) -> Result<()> {
    for (name, scope) in sidecars() {
        ensure_sidecar(root, name, scope)?;
    }
    Ok(())
}

fn ensure_sidecar(root: &Path, name: &str, scope: config::Scope) -> Result<()> {
    let path = root.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{name}: {e}"))?;
    }
    let action = match read_json(&path) {
        Ok(existing) => {
            let fresh = config::migrate_singbox(&existing, scope);
            if fresh == existing {
                config::check_singbox(&path).map_err(|e| format!("{name}: {e}"))?;
                out::status("→", &format!("{name}: already current"));
                return Ok(());
            }
            write_json(&path, &fresh)?;
            "migrated"
        }
        Err(_) => {
            write_json(&path, &config::singbox_config(scope))?;
            "created"
        }
    };
    out::status("→", &format!("{name}: {action}"));
    Ok(())
}

pub fn run_app(root: &Path, index: u16, trailing: &[String], verbose: bool) -> Result<()> {
    if trailing.is_empty() {
        return Err(
            "usage: xvpn <n> <app> [args...]\n  n=0 runs without proxy, n>0 runs through that profile temporarily"
                .into(),
        );
    }
    let app = &trailing[0];
    let app_args = &trailing[1..];

    let (name, cfg, socks_port, http_port) = if index == 0 {
        (
            "direct".to_string(),
            config::direct_xray_config(10812, 10813),
            10812,
            10813,
        )
    } else {
        let profiles = profile::list(root);
        let profile = profiles.get(index as usize - 1).ok_or_else(|| {
            format!(
                "no profile {index} (have {}; see `xvpn profile list`)",
                profiles.len()
            )
        })?;
        let mut cfg = read_json(&profile.path)?;
        let socks_port = 10810;
        let http_port = 10811;

        if let Some(inbounds) = cfg.get_mut("inbounds").and_then(|i| i.as_array_mut()) {
            for inbound in inbounds {
                if inbound["protocol"] == "socks" {
                    inbound["port"] = serde_json::json!(socks_port);
                } else if inbound["protocol"] == "http" {
                    inbound["port"] = serde_json::json!(http_port);
                }
            }
        }
        (profile.name.clone(), cfg, socks_port, http_port)
    };

    let tmp_dir = std::env::temp_dir().join("xvpn");
    let _ = fs::create_dir_all(&tmp_dir);
    let tmp_id = format!("{}-{}", std::process::id(), index);
    let tmp_path = tmp_dir.join(format!("xray-isolated-{}.json", tmp_id));
    write_json(&tmp_path, &cfg)?;

    if verbose {
        if index > 0 {
            out::status("✓", &format!("active profile: {name}"));
        }
        out::status(
            "→",
            &format!("starting isolated proxy on ports {socks_port}/{http_port}"),
        );
    }

    let log_path = tmp_dir.join(format!("xray-isolated-{}.log", tmp_id));
    let log_file = fs::File::create(&log_path).map_err(|e| format!("creating log: {e}"))?;

    let mut xray = Command::new("xray")
        .args(["run", "-c"])
        .arg(&tmp_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log_file)
        .spawn()
        .map_err(|e| format!("could not start xray: {e}"))?;

    let mut ready = false;
    for _ in 0..50 {
        if TcpStream::connect(format!("127.0.0.1:{socks_port}")).is_ok() {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(Some(status)) = xray.try_wait() {
            let log_content = fs::read_to_string(&log_path).unwrap_or_default();
            let _ = fs::remove_file(&tmp_path);
            let _ = fs::remove_file(&log_path);
            return Err(format!("xray exited with {status}\n{log_content}"));
        }
    }

    if !ready {
        let _ = xray.kill();
        let _ = fs::remove_file(&tmp_path);
        let _ = fs::remove_file(&log_path);
        return Err("timed out waiting for isolated xray to start".into());
    }

    if verbose {
        out::status(
            "→",
            &format!(
                "running {app} via {}",
                if index == 0 {
                    "direct bypass".to_string()
                } else {
                    format!("profile {index}")
                }
            ),
        );
    }

    let mut cmd = Command::new(app);
    cmd.args(app_args);
    cmd.stdin(Stdio::inherit());
    if out::is_quiet() {
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
    } else {
        cmd.stdout(Stdio::inherit());
        cmd.stderr(Stdio::inherit());
    }
    cmd.env("ALL_PROXY", format!("socks5://127.0.0.1:{socks_port}"));
    cmd.env("all_proxy", format!("socks5://127.0.0.1:{socks_port}"));
    cmd.env("HTTP_PROXY", format!("http://127.0.0.1:{http_port}"));
    cmd.env("http_proxy", format!("http://127.0.0.1:{http_port}"));
    cmd.env("HTTPS_PROXY", format!("http://127.0.0.1:{http_port}"));
    cmd.env("https_proxy", format!("http://127.0.0.1:{http_port}"));

    let status = cmd
        .spawn()
        .map_err(|e| format!("could not run {app}: {e}"))?
        .wait()
        .map_err(|e| format!("{app} failed: {e}"))?;

    let _ = xray.kill();
    let _ = fs::remove_file(&tmp_path);
    let _ = fs::remove_file(&log_path);

    if status.success() {
        Ok(())
    } else {
        Err(format!("{app} exited with {status}"))
    }
}
