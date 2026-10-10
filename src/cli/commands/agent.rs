use std::fs;
use std::path::Path;
use std::process::Command;

use crate::platform::network;
use crate::utils::output as out;
use crate::vpn::config::{self};
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
            let id = format!("gui/{}/com.xvpn", unsafe { libc::getuid() });
            launchctl_bootout(&id)?;
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
            let id = format!("gui/{}/com.xvpn", unsafe { libc::getuid() });
            launchctl(&["kickstart".into(), "-k".into(), id])
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