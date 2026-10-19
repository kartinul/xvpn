use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::platform::network;
use crate::utils::output as out;
use crate::vpn::config::{self};
use crate::vpn::supervisor;
use crate::Result;

pub fn agent(cmd: crate::cli::AgentCmd, _root: &Path) -> Result<()> {
    let binary = std::env::current_exe().map_err(|e| format!("locating xvpn: {e}"))?;
    match cmd {
        crate::cli::AgentCmd::Install => {
            out::status("→", "installing and loading user agent");
            install_agent(&binary)
        }
        crate::cli::AgentCmd::Stop => {
            out::status("→", "stopping supervisor and proxies");
            stop_agent()
        }
        crate::cli::AgentCmd::Restart => {
            out::status("→", "restarting supervisor");
            restart_agent(&binary)
        }
    }
}

fn launchctl(args: &[String]) -> Result<()> {
    let output = Command::new("launchctl")
        .args(args)
        .output()
        .map_err(|e| format!("launchctl failed: {e}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if detail.is_empty() {
            format!("launchctl exited with {}", output.status)
        } else {
            format!("launchctl failed: {detail}")
        });
    }
    Ok(())
}

fn launchctl_bootout(id: &str) -> Result<()> {
    let output = Command::new("launchctl")
        .args(["bootout", id])
        .output()
        .map_err(|e| format!("launchctl failed: {e}"))?;
    if output.status.success() || output.status.code() == Some(3) {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if detail.is_empty() {
        format!("launchctl exited with {}", output.status)
    } else {
        format!("launchctl failed: {detail}")
    })
}

fn install_agent(binary: &Path) -> Result<()> {
    match network::os() {
        network::Os::MacOs => {
            let dir = config::home_dir().join("Library/LaunchAgents");
            fs::create_dir_all(&dir).map_err(|e| format!("creating LaunchAgents: {e}"))?;
            let plist = dir.join("com.xvpn.plist");
            let log = config::log_file("supervisor");
            let content = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict>\
<key>Label</key><string>com.xvpn</string><key>ProgramArguments</key><array>\
<string>{}</string><string>--supervise</string></array><key>RunAtLoad</key><true/>\
<key>EnvironmentVariables</key><dict><key>PATH</key><string>\
/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string></dict>\
<key>KeepAlive</key><true/><key>StandardOutPath</key><string>{}</string>\
<key>StandardErrorPath</key><string>{}</string></dict></plist>",
                binary.display(),
                log.display(),
                log.display()
            );
            fs::write(&plist, content).map_err(|e| format!("writing agent: {e}"))?;
            install_root_setup()?;
            let id = format!("gui/{}/com.xvpn", unsafe { libc::getuid() });
            launchctl_bootout(&id)?;
            // bootout returns before launchd finishes unloading; bootstrap too early fails with EIO
            for _ in 0..50 {
                if launchctl(&["print".into(), id.clone()]).is_err() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            launchctl(&[
                "bootstrap".into(),
                format!("gui/{}", unsafe { libc::getuid() }),
                plist.display().to_string(),
            ])?;
            launchctl(&["enable".into(), id])?;
            out::status("✓", "user agent loaded");
        }
        network::Os::Linux => {
            let dir = config::home_dir().join(".config/systemd/user");
            fs::create_dir_all(&dir).map_err(|e| format!("creating systemd user dir: {e}"))?;
            let unit = dir.join("xvpn.service");
            let content = format!(
                "[Unit]\nDescription=xvpn supervisor\nAfter=network-online.target\n\
Wants=network-online.target\n\n[Service]\nType=simple\nExecStart={} --supervise\n\
Restart=always\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
                binary.display()
            );
            fs::write(&unit, content).map_err(|e| format!("writing systemd unit: {e}"))?;
            command_ok("systemctl", &["--user", "daemon-reload"])?;
            command_ok("systemctl", &["--user", "enable", "--now", "xvpn.service"])?;
        }
    }
    Ok(())
}

fn sudoers_rule(user: &str) -> String {
    let pattern = supervisor::root_pattern();
    format!(
        "{user} ALL=(root) NOPASSWD: {} run -c {}\n\
         {user} ALL=(root) NOPASSWD: /usr/bin/pkill -TERM -f {pattern}\n\
         {user} ALL=(root) NOPASSWD: /usr/bin/pkill -KILL -f {pattern}\n",
        config::ROOT_SING_BOX,
        config::ROOT_ON
    )
}

fn sudo_write(path: &str, content: &str) -> Result<()> {
    let mut child = Command::new("sudo")
        .args(["tee", path])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|e| format!("sudo tee {path}: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("sudo tee: no stdin")?
        .write_all(content.as_bytes())
        .map_err(|e| format!("writing {path}: {e}"))?;
    let status = child.wait().map_err(|e| format!("sudo tee {path}: {e}"))?;
    if !status.success() {
        return Err(format!("could not write {path}"));
    }
    Ok(())
}

// launchd agents have no tty, so they can't reuse a terminal's sudo ticket;
// root only ever runs root-owned copies, never files the user can edit
fn install_root_setup() -> Result<()> {
    let user = std::env::var("USER").map_err(|_| "USER is not set".to_string())?;
    let sing_box = Command::new("which")
        .arg("sing-box")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .ok_or("sing-box not found — run ./install.sh")?;
    let sing_box = fs::canonicalize(&sing_box)
        .map_err(|e| format!("{sing_box}: {e}"))?
        .display()
        .to_string();
    let on_json = serde_json::to_string_pretty(&config::singbox_config(
        config::Scope::Global,
        config::Ingress::Tun,
    ))
    .map_err(|e| format!("rendering on.json: {e}"))?;
    let dir = Path::new(config::ROOT_ON)
        .parent()
        .ok_or("bad ROOT_ON")?
        .display()
        .to_string();
    let draft = "/etc/sudoers.d/xvpn.new"; // sudo skips includedir files with a dot

    out::status(
        "→",
        "installing root-owned sing-box and sudo rule (asks for sudo once)",
    );
    command_ok(
        "sudo",
        &[
            "install", "-d", "-m", "755", "-o", "root", "-g", "wheel", &dir,
        ],
    )?;
    command_ok(
        "sudo",
        &[
            "install",
            "-m",
            "755",
            "-o",
            "root",
            "-g",
            "wheel",
            &sing_box,
            config::ROOT_SING_BOX,
        ],
    )?;
    sudo_write(config::ROOT_ON, &on_json)?;
    sudo_write(draft, &sudoers_rule(&user))?;
    if let Err(e) = command_ok("sudo", &["visudo", "-cf", draft]) {
        let _ = command_ok("sudo", &["rm", "-f", draft]);
        return Err(e);
    }
    command_ok("sudo", &["chmod", "440", draft])?;
    command_ok("sudo", &["mv", draft, "/etc/sudoers.d/xvpn"])?;
    for program in ["sing-box", "xray"] {
        let log = config::log_file(program);
        if log.exists() {
            let _ = command_ok("sudo", &["chown", &user, &log.display().to_string()]);
        }
    }
    Ok(())
}

fn stop_agent() -> Result<()> {
    match network::os() {
        network::Os::MacOs => {
            let id = format!("gui/{}/com.xvpn", unsafe { libc::getuid() });
            launchctl_bootout(&id)?;
        }
        network::Os::Linux => {
            command_ok("systemctl", &["--user", "disable", "--now", "xvpn.service"])?;
        }
    }
    Ok(())
}

fn restart_agent(binary: &Path) -> Result<()> {
    match network::os() {
        network::Os::MacOs => {
            let domain = format!("gui/{}", unsafe { libc::getuid() });
            if launchctl(&[
                "kickstart".into(),
                "-k".into(),
                format!("{domain}/com.xvpn"),
            ])
            .is_ok()
            {
                return Ok(());
            }
            let plist = config::home_dir().join("Library/LaunchAgents/com.xvpn.plist");
            if !plist.is_file() {
                return Err("agent not installed — run `xvpn agent install`".into());
            }
            launchctl(&["bootstrap".into(), domain, plist.display().to_string()])
        }
        network::Os::Linux => {
            install_agent(binary)?;
            command_ok("systemctl", &["--user", "restart", "xvpn.service"])
        }
    }
}

pub fn command_ok(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|e| format!("{program} failed: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sudoers_rule_pins_root_owned_files() {
        let rule = sudoers_rule("me");
        assert_eq!(
            rule,
            "me ALL=(root) NOPASSWD: /usr/local/libexec/xvpn/sing-box run -c /usr/local/libexec/xvpn/on.json\n\
             me ALL=(root) NOPASSWD: /usr/bin/pkill -TERM -f ^/usr/local/libexec/xvpn/sing-box run -c /usr/local/libexec/xvpn/on.json\n\
             me ALL=(root) NOPASSWD: /usr/bin/pkill -KILL -f ^/usr/local/libexec/xvpn/sing-box run -c /usr/local/libexec/xvpn/on.json\n"
        );
    }
}
