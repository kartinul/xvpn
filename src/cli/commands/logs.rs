use std::fs;
use std::process::{Command, Stdio};

use crate::vpn::config::{self};
use crate::Result;

pub fn logs() -> Result<()> {
    let logs = [
        config::log_file("xray"),
        config::log_file("sing-box"),
        config::log_file("supervisor"),
    ];
    let mut args = vec!["-f".to_string()];
    for log in &logs {
        if let Some(parent) = log.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::OpenOptions::new().create(true).append(true).open(log);
        args.push(log.to_string_lossy().to_string());
    }
    let mut cmd = Command::new("tail");
    cmd.args(&args);
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());
    let status = cmd
        .spawn()
        .map_err(|e| format!("could not tail logs: {e}"))?
        .wait()
        .map_err(|e| format!("tail logs failed: {e}"))?;
    std::process::exit(status.code().unwrap_or(1));
}
