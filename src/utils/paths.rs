use std::fs;
use std::path::{Path, PathBuf};

pub use crate::vpn::config::migrate_state;

pub const MODE: &str = "mode";
pub const XRAY: &str = "xray.json";
pub const ON: &str = "modes/on.json";
pub const DEFAULT: &str = "modes/default.json";
pub const NETWORKS: &str = "modes/networks";
pub const PROFILES: &str = "profiles";
pub const ACTIVE: &str = "active";
pub const ORDER: &str = "order";

fn config_home() -> PathBuf {
    if let Some(dir) = env_var("XDG_CONFIG_HOME") {
        return dir;
    }
    home().join(".config")
}

fn state_home() -> PathBuf {
    if let Some(dir) = env_var("XDG_STATE_HOME") {
        return dir;
    }
    home().join(".local").join("state")
}

fn home() -> PathBuf {
    #[cfg(unix)]
    if let Some(dir) = sudo_user_home() {
        return dir;
    }
    env_var("HOME").unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(unix)]
fn sudo_user_home() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let sudo_user = std::env::var_os("SUDO_USER")?;
    let c_user = std::ffi::CString::new(sudo_user.as_bytes()).ok()?;
    unsafe {
        let pwd = libc::getpwnam(c_user.as_ptr());
        if !pwd.is_null() && !(*pwd).pw_dir.is_null() {
            let dir = std::ffi::CStr::from_ptr((*pwd).pw_dir);
            if let Ok(dir_str) = dir.to_str() {
                return Some(PathBuf::from(dir_str));
            }
        }
    }
    None
}

pub fn env_var(key: &str) -> Option<PathBuf> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

pub fn state_file() -> PathBuf {
    if let Some(p) = env_var("XVPN_STATE") {
        return p;
    }
    state_home().join("xvpn").join("current")
}

pub fn lock_file() -> PathBuf {
    if let Some(p) = env_var("XVPN_LOCK") {
        return p;
    }
    state_home().join("xvpn").join("supervisor.lock")
}

pub fn log_file(program: &str) -> PathBuf {
    state_home().join("xvpn").join(format!("{program}.log"))
}

pub fn root() -> PathBuf {
    if let Some(dir) = env_var("XVPN_DIR") {
        return dir;
    }
    let primary = config_home().join("xvpn");
    if is_writable(&primary) {
        primary
    } else {
        home().join(".xvpn")
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn is_writable(dir: &Path) -> bool {
    let _ = fs::create_dir_all(dir);
    let probe = dir.join(".xvpn_write_test");
    let _ = fs::remove_file(&probe);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map(|f| {
            drop(f);
            let _ = fs::remove_file(&probe);
            true
        })
        .unwrap_or(false)
}

pub fn legacy_roots() -> Vec<PathBuf> {
    [
        PathBuf::from("/usr/local/etc/xvpn"),
        PathBuf::from("/usr/local/xvpn"),
        home().join(".xvpn"),
    ]
    .into_iter()
    .collect()
}

pub fn stale_root_notice() -> Option<String> {
    if env_var("XVPN_DIR").is_some() {
        return None;
    }
    let current = root();
    if current.join(PROFILES).is_dir() || current.join(MODE).is_file() {
        return None;
    }
    legacy_roots()
        .into_iter()
        .find(|old| old != &current && (old.join(MODE).is_file() || old.join(PROFILES).is_dir()))
        .map(|old| {
            format!(
                "found xvpn state in {} but this build uses {}\n\
                 run `xvpn migrate` to move it across",
                old.display(),
                current.display()
            )
        })
}

pub fn profiles_dir(root: &Path) -> PathBuf {
    root.join(PROFILES)
}

pub fn active_file(root: &Path) -> PathBuf {
    root.join(ACTIVE)
}

pub fn order_file(root: &Path) -> PathBuf {
    root.join(ORDER)
}

pub fn is_root() -> bool {
    crate::vpn::config::is_root()
}
