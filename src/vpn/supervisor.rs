use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::platform::network;
use crate::vpn::config::{self, Ingress};
use crate::vpn::profile;
use crate::Result;

const POLL: Duration = Duration::from_secs(5);
const STARTUP_GRACE: Duration = Duration::from_secs(3);
const REVALIDATE_EVERY: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Want {
    None,
    On,
    Selective,
    Failed,
}

impl Want {
    pub fn as_str(self) -> &'static str {
        match self {
            Want::None => "none",
            Want::On => "on",
            Want::Selective => "selective",
            Want::Failed => "failed",
        }
    }

    fn sidecar(self) -> Option<&'static str> {
        match self {
            Want::On => Some(config::ON),
            Want::Selective => Some(config::DEFAULT),
            _ => None,
        }
    }
}

struct Proxy {
    child: Child,
    started: Instant,
    elevated: bool,
}

impl Proxy {
    fn has_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    fn past_grace(&self) -> bool {
        self.started.elapsed() > STARTUP_GRACE
    }
}

pub struct Plan {
    pub want: Want,
    pub key: String,
}

pub fn plan(root: &Path, sudo_cache: &mut Option<(Instant, bool)>) -> Plan {
    let mode_path = root.join(config::MODE);
    let mode = fs::read_to_string(&mode_path)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "default".to_string());
    let mut key = format!("{mode}:{}", mtime(&mode_path));

    let active = match profile::resolve_active(root) {
        Ok((name, path)) => {
            key.push_str(&format!(":{name}:{}", mtime(&path)));
            Some(name)
        }
        Err(e) => {
            if mode == "off" {
                None
            } else {
                return Plan {
                    want: Want::Failed,
                    key: format!("failed:{e}"),
                };
            }
        }
    };

    let want = if active.is_none() {
        Want::None
    } else if mode == "on" {
        Want::On
    } else if mode == "off" {
        Want::None
    } else if network::read_networks(root).matches(&network::dns_servers()) {
        Want::Selective
    } else {
        Want::None
    };

    if let Some(sidecar) = want.sidecar() {
        let path = root.join(sidecar);
        if !path.is_file() {
            return Plan {
                want: Want::Failed,
                key: format!("failed:{sidecar} missing"),
            };
        }
        key.push_str(&format!(":{sidecar}:{}", mtime(&path)));

        if ingress() == Ingress::Tun && !config::is_root() {
            let ready = cached_sudo(sudo_cache);
            key.push_str(&format!(":sudo:{ready}"));
            if !ready {
                return Plan {
                    want: Want::Failed,
                    key: "failed:this mode needs root for the tun interface — \
                      run `xvpn supervise` in foreground or configure sing-box in sudoers"
                        .to_string(),
                };
            }
        }
    }

    Plan { want, key }
}

fn ingress() -> Ingress {
    if network::supports_gateway() {
        Ingress::TProxy
    } else {
        Ingress::Tun
    }
}

fn mtime(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn state_path() -> PathBuf {
    config::state_file()
}

fn publish(state: &str) {
    let path = state_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            log(format!("cannot create {}: {e}", parent.display()));
            return;
        }
    }
    if let Err(e) = fs::write(&path, format!("{state}\n")) {
        log(format!("cannot write {}: {e}", path.display()));
    }
}

fn log(msg: impl std::fmt::Display) {
    eprintln!("[xvpn] {msg}");
}

struct SingleInstance {
    _file: fs::File,
}

fn single_instance() -> Result<SingleInstance> {
    let path = config::lock_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let rc = unsafe {
        libc::flock(
            std::os::unix::io::AsRawFd::as_raw_fd(&file),
            libc::LOCK_EX | libc::LOCK_NB,
        )
    };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::WouldBlock {
            return Err(format!(
                "another xvpn supervisor is already running ({} is locked)\n\
                 stop it first: pkill -f 'xvpn --supervise'",
                path.display()
            ));
        }
        return Err(format!("locking {}: {err}", path.display()));
    }
    Ok(SingleInstance { _file: file })
}

pub fn is_running() -> bool {
    let path = config::lock_file();
    let Ok(file) = fs::OpenOptions::new().read(true).write(true).open(&path) else {
        return false;
    };
    let rc = unsafe {
        libc::flock(
            std::os::unix::io::AsRawFd::as_raw_fd(&file),
            libc::LOCK_EX | libc::LOCK_NB,
        )
    };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::WouldBlock
            || err.raw_os_error() == Some(libc::EWOULDBLOCK)
            || err.raw_os_error() == Some(libc::EAGAIN)
        {
            return true;
        }
    } else {
        unsafe {
            libc::flock(std::os::unix::io::AsRawFd::as_raw_fd(&file), libc::LOCK_UN);
        }
    }
    false
}

fn sudo_ready() -> bool {
    let check_true = Command::new("sudo")
        .args(["-n", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if check_true {
        return true;
    }
    Command::new("sudo")
        .args(["-n", "sing-box", "version"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn cached_sudo(cache: &mut Option<(Instant, bool)>) -> bool {
    let now = Instant::now();
    if let Some((ts, ready)) = *cache {
        if now.duration_since(ts) < Duration::from_secs(300) {
            return ready;
        }
    }
    let ready = sudo_ready();
    *cache = Some((now, ready));
    ready
}

fn sidecar_needs_root() -> bool {
    ingress() == Ingress::Tun && !config::is_root()
}

fn mode_needs_root(root: &Path, mode: &str) -> bool {
    if config::is_root() || ingress() != Ingress::Tun {
        return false;
    }
    mode == "on"
        || (mode == "default" && network::read_networks(root).matches(&network::dns_servers()))
}

pub fn authorize_transition(root: &Path, mode: &str) -> Result<()> {
    let current = fs::read_to_string(state_path())
        .map(|state| matches!(state.trim(), "on" | "selective"))
        .unwrap_or(false)
        || fs::read_to_string(root.join(config::MODE))
            .map(|current| mode_needs_root(root, current.trim()))
            .unwrap_or(false);
    if !mode_needs_root(root, mode) && !(mode == "off" && current && sidecar_needs_root()) {
        return Ok(());
    }
    let status = Command::new("sudo")
        .arg("-v")
        .status()
        .map_err(|e| format!("could not request sudo authorization: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("sudo authorization is required for this transition".into())
    }
}

fn spawn(program: &str, config_path: &Path, elevate: bool) -> Result<Proxy> {
    let log_path = config::log_file(program);
    if let Some(parent) = log_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let file = match fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        Ok(f) => Some(f),
        Err(e) => {
            log(format!(
                "cannot open {}: {e} — {program} output will be discarded",
                log_path.display()
            ));
            None
        }
    };
    let stderr = file
        .as_ref()
        .map(|f| Stdio::from(f.try_clone().unwrap()))
        .unwrap_or_else(Stdio::null);

    let mut command = if elevate {
        let mut c = Command::new("sudo");
        c.args(["-n", program]);
        c
    } else {
        Command::new(program)
    };
    let child = command
        .args(["run", "-c"])
        .arg(config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .map_err(|e| format!("could not start {program}: {e}"))?;

    Ok(Proxy {
        child,
        started: Instant::now(),
        elevated: elevate,
    })
}

#[cfg(unix)]
fn hook_shutdown_signals(flag: &Arc<AtomicBool>) {
    use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
    for signal in [SIGTERM, SIGINT, SIGHUP] {
        let _ = signal_hook::flag::register(signal, Arc::clone(flag));
    }
}

#[cfg(not(unix))]
fn hook_shutdown_signals(_flag: &Arc<AtomicBool>) {}

pub fn supervise(root: &Path) -> Result<()> {
    if let Err(e) = crate::cli::handlers::ensure_dependencies() {
        eprintln!("[xvpn] dependency error: {}", e);
        eprintln!("[xvpn] aborting supervisor start – run ./install.sh to install missing tools.");
        return Err(e);
    }
    let shutdown = Arc::new(AtomicBool::new(false));
    hook_shutdown_signals(&shutdown);

    let _instance = single_instance()?;

    log(format!("supervising {}", root.display()));
    if let Some(notice) = config::stale_root_notice() {
        log(notice);
    }
    let mut running: BTreeMap<&'static str, Proxy> = BTreeMap::new();
    let mut applied: Option<String> = None;
    let mut last_validated = Instant::now() - REVALIDATE_EVERY;
    let mut sudo_cache: Option<(Instant, bool)> = None;

    while !shutdown.load(Ordering::SeqCst) {
        let plan = plan(root, &mut sudo_cache);
        let key = plan.key.clone();

        let stale: Vec<(&'static str, &'static str)> = running
            .iter_mut()
            .filter_map(|(name, p)| {
                if p.past_grace() && p.has_exited() {
                    Some((*name, "exited unexpectedly"))
                } else {
                    None
                }
            })
            .collect();
        for (name, reason) in &stale {
            log(format!("{name}: {reason}"));
            running.remove(name);
            applied = None;
        }

        let changed = applied.as_deref() != Some(key.as_str());
        let revalidate = last_validated.elapsed() >= REVALIDATE_EVERY;

        if changed || revalidate {
            let next_applied;
            if plan.want == Want::Failed {
                if changed {
                    log(format!(
                        "config rejected, staying down: {}",
                        key.trim_start_matches("failed:")
                    ));
                }
                let _ = stop_all(&mut running);
                publish("failed");
                next_applied = Some(key);
            } else {
                if changed {
                    log(format!("applying {}", plan.want.as_str()));
                    if stop_all(&mut running) && start_for(root, plan.want, &mut running) {
                        publish(plan.want.as_str());
                        next_applied = Some(key);
                    } else {
                        publish("failed");
                        next_applied = None;
                    }
                } else {
                    publish(plan.want.as_str());
                    next_applied = Some(key);
                }
            }
            applied = next_applied;
            last_validated = Instant::now();
        }

        interruptible_sleep(POLL, &shutdown);
    }

    log("shutting down");
    let _ = stop_all(&mut running);
    let mode_path = root.join(config::MODE);
    let _ = fs::write(&mode_path, b"off\n");
    let _ = fs::remove_file(state_path());
    log("stopped");
    Ok(())
}

fn interruptible_sleep(total: Duration, shutdown: &AtomicBool) {
    const STEP: Duration = Duration::from_millis(200);
    let mut waited = Duration::ZERO;
    while waited < total {
        if shutdown.load(Ordering::SeqCst) {
            return;
        }
        std::thread::sleep(STEP);
        waited += STEP;
    }
}

fn start_for(root: &Path, want: Want, running: &mut BTreeMap<&'static str, Proxy>) -> bool {
    if want == Want::None {
        return true;
    }
    let Ok((name, _)) = profile::active_config(root) else {
        return false;
    };
    log(format!("profile: {name}"));

    match spawn("xray", &root.join(config::XRAY), false) {
        Ok(p) => {
            running.insert("xray", p);
        }
        Err(e) => {
            log(e);
            publish("failed");
            return false;
        }
    }

    let Some(sidecar) = want.sidecar() else {
        return true;
    };
    let elevate = sidecar_needs_root();
    if elevate {
        log("tun mode needs root — starting sing-box with cached sudo");
    }
    match spawn("sing-box", &root.join(sidecar), elevate) {
        Ok(p) => {
            running.insert("sing-box", p);
            true
        }
        Err(e) => {
            log(e);
            let _ = stop_all(running);
            publish("failed");
            false
        }
    }
}

fn stop_all(running: &mut BTreeMap<&'static str, Proxy>) -> bool {
    let mut stopped = true;
    for (_, mut proxy) in std::mem::take(running) {
        if proxy.elevated {
            let result = Command::new("sudo")
                .args(["-n", "pkill", "-x", "sing-box"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let child_exited = proxy.child.try_wait().ok().flatten().is_some();
            if !result.map(|status| status.success()).unwrap_or(false) && !child_exited {
                log("could not stop root-owned sing-box; run `sudo -v` and retry");
                stopped = false;
            }
        }
        let _ = proxy.child.kill();
        let _ = proxy.child.wait();
    }
    let _ = fs::remove_file(state_path());
    stopped
}
