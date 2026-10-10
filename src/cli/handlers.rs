use std::fs;
use std::io::{self, Write};
use std::net::{IpAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use dialoguer::MultiSelect;

use crate::cli::{AppsAction, Mode, ProfileCmd, SitesAction};
use crate::platform::network;
use crate::utils::output as out;
use crate::utils::{validate_apps_args, validate_sites_args};
use crate::vpn::config::{self, Ingress, Scope};
use crate::vpn::profile;
use crate::vpn::supervisor;
use crate::{Error, Result};

pub fn use_profile(root: &Path, index: u16) -> Result<()> {
    profile::migrate_legacy(root)?;
    if index == 0 {
        return set_mode(root, Mode::Off);
    }
    let mode = read_mode(root);
    let profiles = profile::list(root);
    let name = profiles
        .get(index as usize - 1)
        .map(|p| p.name.clone())
        .ok_or_else(|| {
            format!(
                "no profile {index} (have {}; see `xvpn profile list`)",
                profiles.len()
            )
        })?;
    let target_mode = mode_after_profile_selection(&mode);
    if target_mode.as_str() == mode {
        supervisor::authorize_profile_transition(root, target_mode.as_str())?;
    }
    profile::activate(root, &name)?;
    out::status("✓", &format!("active profile: {name}"));

    if target_mode.as_str() != mode {
        set_mode(root, target_mode)?;
    } else {
        refresh_sidecars(root)?;
        set_mode(root, target_mode)?;
    }

    Ok(())
}

pub(crate) fn mode_after_profile_selection(mode: &str) -> Mode {
    match mode {
        "off" | "on" => Mode::On,
        "default" => Mode::Default,
        _ => Mode::Default,
    }
}

pub fn run(result: Result<()>) {
    if let Err(e) = result {
        if !e.is_empty() {
            eprintln!("Error: {e}");
        }
        std::process::exit(1);
    }
}

fn read_mode(root: &Path) -> String {
    fs::read_to_string(root.join(config::MODE))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unset".to_string())
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

pub fn migrate() -> Result<()> {
    match config::migrate_state()? {
        Some(from) => out::status(
            "✓",
            &format!(
                "migrated state from {} to {}",
                from.display(),
                config::root().display()
            ),
        ),
        None => out::status("→", "nothing to migrate"),
    }
    Ok(())
}

fn print_routing(root: &Path) {
    let path = root.join(config::DEFAULT);
    let cfg = match config::read_json(&path) {
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

pub fn import(root: &Path, link: Option<&str>, force: bool) -> Result<()> {
    profile::migrate_legacy(root)?;
    let link = read_link(link)?;
    let name = profile::import(root, None, &link, force)?;
    out::status("✓", &format!("saved profile '{name}'"));
    refresh_sidecars(root)?;
    out::status("→", "active. the supervisor will reload");
    Ok(())
}

pub fn profile(root: &Path, cmd: ProfileCmd) -> Result<()> {
    profile::migrate_legacy(root)?;
    match cmd {
        ProfileCmd::Add { name, link, force } => {
            let link = read_link(link.as_deref())?;
            let saved = profile::import(root, Some(&name), &link, force)?;
            out::status("✓", &format!("saved profile '{saved}'"));
            refresh_sidecars(root)?;
            out::status("→", "active. the supervisor will reload");
            Ok(())
        }
        ProfileCmd::List => profile::print_list(root),
        ProfileCmd::Use { name } => {
            let name = profile::resolve(root, &name)?;
            profile::validate(root, &name)?;
            profile::activate(root, &name)?;
            out::status("✓", &format!("active profile: {name}"));
            refresh_sidecars(root)?;
            out::status("→", "supervisor will reload");
            Ok(())
        }
        ProfileCmd::Remove { name } => {
            let name = profile::resolve(root, &name)?;
            profile::remove(root, &name)?;
            out::status("✓", &format!("removed profile '{name}'"));
            Ok(())
        }
        ProfileCmd::Show { name } => {
            let name = match name {
                Some(n) => profile::resolve(root, &n)?,
                None => match profile::active_name(root) {
                    Some(n) => n,
                    None => return Err("no profiles configured".into()),
                },
            };
            let cfg = config::read_json(&profile::path_for(root, &name))?;
            println!("{}", serde_json::to_string_pretty(&cfg).unwrap());
            Ok(())
        }
    }
}

fn read_link(link: Option<&str>) -> Result<String> {
    match link {
        Some(l) => Ok(l.trim().to_string()),
        None => {
            print!("Paste vless:// link: ");
            io::stdout().flush().map_err(|e| e.to_string())?;
            let mut input = String::new();
            io::stdin()
                .read_line(&mut input)
                .map_err(|e| format!("reading input: {e}"))?;
            Ok(input.trim().to_string())
        }
    }
}

pub fn apps(root: &Path, action: AppsAction, app: Option<String>) -> Result<()> {
    let (action, app) = validate_apps_args(action, app)?;
    match (action, app) {
        (AppsAction::Show, _) => list_apps(root),
        (AppsAction::Set, Some(app)) => add_app(root, &app),
        (AppsAction::Remove, Some(app)) => remove_app(root, &app),
        _ => Err("invalid apps arguments".into()),
    }
}

pub fn list_apps(root: &Path) -> Result<()> {
    let apps = network::listed_apps(&root.join(config::DEFAULT))?;
    out::section("apps:");
    out::list(&apps.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    Ok(())
}

pub fn add_app(root: &Path, app: &str) -> Result<()> {
    edit_app(root, true, app)
}

pub fn remove_app(root: &Path, app: &str) -> Result<()> {
    edit_app(root, false, app)
}

fn edit_app(root: &Path, add: bool, app: &str) -> Result<()> {
    let shown = network::normalize_app(app)?;
    announce_edit(
        network::edit_app(&root.join(config::DEFAULT), add, &shown),
        add,
        &shown,
    )
}

pub fn sites(root: &Path, action: SitesAction, site: Option<String>) -> Result<()> {
    let (action, site) = validate_sites_args(action, site)?;
    match (action, site) {
        (SitesAction::Show, _) => list_sites(root),
        (SitesAction::Set, Some(site)) => add_site(root, &site),
        (SitesAction::Remove, Some(site)) => remove_site(root, &site),
        _ => Err("invalid sites arguments".into()),
    }
}

pub fn list_sites(root: &Path) -> Result<()> {
    let sites = network::listed_sites(&root.join(config::DEFAULT))?;
    out::section("sites (and subdomains):");
    out::list(&sites.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    Ok(())
}

pub fn add_site(root: &Path, site: &str) -> Result<()> {
    edit_site(root, true, site)
}

pub fn remove_site(root: &Path, site: &str) -> Result<()> {
    edit_site(root, false, site)
}

fn edit_site(root: &Path, add: bool, site: &str) -> Result<()> {
    let shown = network::normalize_site(site)?;
    announce_edit(
        network::edit_site(&root.join(config::DEFAULT), add, &shown),
        add,
        &shown,
    )
}

fn announce_edit(result: Result<()>, add: bool, name: &str) -> Result<()> {
    match result {
        Ok(()) => {
            out::status(
                if add { "✓" } else { "✗" },
                &format!("{} {}", if add { "Added" } else { "Removed" }, name),
            );
            Ok(())
        }
        Err(e) if network::is_noop(&e) => {
            out::status("→", &e);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

pub fn use_network(root: &Path, add: bool, dns: Option<&str>) -> Result<()> {
    let msg = if add {
        network::register(root)?
    } else {
        network::unregister(root, dns)?
    };
    out::status("✓", &msg);
    Ok(())
}

const PROBES: [(&str, u16); 2] = [("socks5", config::SOCKS_PORT), ("http", config::HTTP_PORT)];
const IP_CHECK_HOST: &str = "ip.me";
const IFCONFIG_CHECK_HOST: &str = "ifconfig.me";
const IPV4_FALLBACK_IP_URL: &str = "https://api4.ipify.org";
const IPV4_FALLBACK_IFCONFIG_URL: &str = "https://ipv4.icanhazip.com";

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

fn ingress() -> Ingress {
    if network::supports_gateway() {
        Ingress::TProxy
    } else {
        Ingress::Tun
    }
}

fn sidecars() -> [(&'static str, Scope); 2] {
    [
        (config::ON, Scope::Global),
        (config::DEFAULT, Scope::Selective),
    ]
}

fn refresh_sidecars(root: &Path) -> Result<()> {
    let ingress = ingress();
    for (name, scope) in sidecars() {
        ensure_sidecar(root, name, scope, ingress)?;
    }
    Ok(())
}

fn ensure_sidecar(root: &Path, name: &str, scope: Scope, ingress: Ingress) -> Result<()> {
    let path = root.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{name}: {e}"))?;
    }
    let action = match config::read_json(&path) {
        Ok(existing) => {
            let fresh = config::migrate_singbox(&existing, scope, ingress);
            if fresh == existing {
                config::check_singbox(&path).map_err(|e| format!("{name}: {e}"))?;
                out::status("→", &format!("{name}: already current"));
                return Ok(());
            }
            config::write_json(&path, &fresh)?;
            "migrated"
        }
        Err(_) => {
            config::write_json(&path, &config::singbox_config(scope, ingress))?;
            "created"
        }
    };
    config::check_singbox(&path).map_err(|e| format!("{name}: {e}"))?;
    out::status("✓", &format!("{name}: {action}, sing-box check OK"));
    Ok(())
}

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

const DIAGNOSTIC_TESTS: [(&str, &str, &str, bool); 6] = [
    ("direct", "xvpn off", "direct connection", true),
    ("sites", "xvpn sites set ip.me", "site routing", false),
    ("profiles", "xvpn <index>", "every profile", true),
    (
        "profile-commands",
        "xvpn <index> <cmd>",
        "both curl commands through every profile",
        true,
    ),
    (
        "modes",
        "xvpn default / xvpn on",
        "routing mode transitions",
        false,
    ),
    (
        "firefox",
        "xvpn apps set Firefox",
        "Firefox app routing",
        false,
    ),
];

const DIAGNOSTIC_LABEL_WIDTH: usize = 30;
const DIAGNOSTIC_DISPLAY_WIDTH: usize = 78;

struct Spinner {
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Spinner {
    fn start() -> Self {
        let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let active = running.clone();
        let thread = std::thread::spawn(move || {
            let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let mut index = 0;
            while active.load(std::sync::atomic::Ordering::Relaxed) {
                eprint!("\r  {} testing...", frames[index % frames.len()]);
                let _ = io::stderr().flush();
                index += 1;
                std::thread::sleep(Duration::from_millis(90));
            }
        });
        Self {
            running,
            thread: Some(thread),
        }
    }

    fn stop(mut self) {
        self.running
            .store(false, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        eprint!("\r\x1b[2K");
        let _ = io::stderr().flush();
    }
}

fn diagnostic_menu_labels() -> Vec<String> {
    DIAGNOSTIC_TESTS
        .iter()
        .map(|(_, command, description, _)| {
            format!("{command:<DIAGNOSTIC_LABEL_WIDTH$} - {description}")
        })
        .collect()
}

struct DiagnosticLog {
    path: PathBuf,
}

impl DiagnosticLog {
    fn new() -> Result<Self> {
        let path = config::log_file("diagnose");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("creating diagnose log directory: {e}"))?;
        }
        Ok(Self { path })
    }

    fn write(&self, message: &str) -> Result<()> {
        use std::fs::OpenOptions;
        use std::io::Write as _;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| format!("diagnose timestamp: {e}"))?
            .as_secs();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("opening diagnose log: {e}"))?;
        writeln!(file, "[{timestamp}] {message}")
            .map_err(|e| format!("writing diagnose log: {e}"))?;
        println!("  {}", truncate_diagnostic_display(message));
        io::stdout()
            .flush()
            .map_err(|e| format!("flushing diagnose output: {e}"))
    }
}

pub fn diagnose(root: &Path, test: Option<&str>, yes: bool) -> Result<()> {
    let selected = match test {
        Some("all") => (0..DIAGNOSTIC_TESTS.len()).collect(),
        Some(name) => vec![diagnostic_index(name)?],
        None => {
            let labels = diagnostic_menu_labels();
            let defaults: Vec<_> = DIAGNOSTIC_TESTS
                .iter()
                .map(|(_, _, _, default)| *default)
                .collect();
            MultiSelect::new()
                .with_prompt("Select diagnose checks (Space toggles, Enter runs)")
                .items(&labels)
                .defaults(&defaults)
                .interact()
                .map_err(|e| format!("selecting diagnose checks: {e}"))?
        }
    };

    let log = DiagnosticLog::new()?;
    log.write(&format!("BEGIN selected={selected:?}"))?;
    if selected
        .iter()
        .any(|index| DIAGNOSTIC_TESTS[*index].0 == "profiles")
    {
        diagnostic_profile_preflight(root, &log, yes)?;
    }
    let mut failures = Vec::new();

    for index in selected {
        let (name, _, description, _) = DIAGNOSTIC_TESTS[index];
        println!("\n[TEST] {description} ({name})");
        log.write(&format!("START test={name}"))?;
        let started = Instant::now();
        let spinner = Spinner::start();
        out::set_quiet(true);
        let result = run_diagnostic_test(root, name, &log);
        out::set_quiet(false);
        spinner.stop();
        log.write(&format!(
            "END test={name} duration_ms={}",
            started.elapsed().as_millis()
        ))?;
        match result {
            Ok(()) => {
                println!("PASS {name}");
                log.write(&format!("PASS test={name}"))?;
            }
            Err(error) => {
                println!("FAIL {name}: {error}");
                log.write(&format!("FAIL test={name} error={error}"))?;
                failures.push(format!("{name}: {error}"));
            }
        }
    }

    let cleanup = set_mode(root, Mode::Off);
    if let Err(error) = &cleanup {
        log.write(&format!("FAIL final cleanup=xvpn off error={error}"))?;
        failures.push(format!("final cleanup: {error}"));
    } else {
        log.write("CLEANUP final=xvpn off")?;
    }

    if failures.is_empty() {
        println!("\nDIAGNOSE PASS");
        log.write("END result=PASS")?;
        Ok(())
    } else {
        println!("\nDIAGNOSE FAILED:");
        for failure in &failures {
            println!("  - {failure}");
        }
        log.write(&format!("END result=FAIL failures={failures:?}"))?;
        Err(String::new())
    }
}

fn diagnostic_index(name: &str) -> Result<usize> {
    DIAGNOSTIC_TESTS
        .iter()
        .position(|(candidate, _, _, _)| *candidate == name)
        .ok_or_else(|| {
            format!(
                "unknown diagnose test '{name}' (use one of: direct, sites, profiles, \
                 profile-commands, modes, firefox, all)"
            )
        })
}

fn run_diagnostic_test(root: &Path, name: &str, log: &DiagnosticLog) -> Result<()> {
    let result = match name {
        "direct" => diagnostic_direct(root, log),
        "sites" => diagnostic_sites(root, log),
        "profiles" => diagnostic_profiles(root, log),
        "profile-commands" => diagnostic_profile_commands(root, log),
        "modes" => diagnostic_modes(root, log),
        "firefox" => diagnostic_firefox(root, log),
        _ => Err(format!("unknown diagnose test '{name}'")),
    };
    let cleanup = set_mode(root, Mode::Off);
    log.write(if cleanup.is_ok() {
        "CLEANUP test=xvpn off result=PASS"
    } else {
        "CLEANUP test=xvpn off result=FAIL"
    })?;
    result.and(cleanup)
}

fn diagnostic_direct(root: &Path, log: &DiagnosticLog) -> Result<()> {
    log.write("COMMAND xvpn off")?;
    set_mode(root, Mode::Off)?;
    check_curls(log).map(|_| ())
}

fn diagnostic_sites(root: &Path, log: &DiagnosticLog) -> Result<()> {
    log.write("COMMAND xvpn off")?;
    set_mode(root, Mode::Off)?;
    let path = root.join(config::DEFAULT);
    let had_site = network::listed_sites(&path)?
        .iter()
        .any(|site| site == IP_CHECK_HOST);
    let mut added = false;
    let result = (|| {
        log.write(&format!("COMMAND xvpn sites set {IP_CHECK_HOST}"))?;
        add_site(root, IP_CHECK_HOST)?;
        added = true;
        log.write("COMMAND xvpn default")?;
        set_mode(root, Mode::Default)?;
        let (ip_me, ifconfig) = check_curls(log)?;
        if ip_me == ifconfig {
            return Err("site routing returned the same public IP for both hosts".into());
        }
        Ok(())
    })();
    let mut cleanup_errors = Vec::new();
    if added && !had_site {
        cleanup_log(
            log,
            &format!("COMMAND xvpn sites remove {IP_CHECK_HOST}"),
            &mut cleanup_errors,
        );
        if let Err(error) = remove_site(root, IP_CHECK_HOST) {
            cleanup_errors.push(error);
        }
    }
    cleanup_log(log, "COMMAND xvpn default", &mut cleanup_errors);
    if let Err(error) = set_mode(root, Mode::Default) {
        cleanup_errors.push(error);
    } else if let Err(error) = check_curls(log).map(|_| ()) {
        cleanup_errors.push(error);
    }
    cleanup_log(log, "COMMAND xvpn off", &mut cleanup_errors);
    if let Err(error) = set_mode(root, Mode::Off) {
        cleanup_errors.push(error);
    }
    match (result, cleanup_errors.is_empty()) {
        (Err(error), _) => Err(error),
        (Ok(()), true) => Ok(()),
        (Ok(()), false) => Err(format!("site cleanup failed: {cleanup_errors:?}")),
    }
}

fn cleanup_log(log: &DiagnosticLog, message: &str, errors: &mut Vec<String>) {
    if let Err(error) = log.write(message) {
        errors.push(error);
    }
}

fn diagnostic_profiles(root: &Path, log: &DiagnosticLog) -> Result<()> {
    log.write("COMMAND xvpn off")?;
    set_mode(root, Mode::Off)?;
    log.write(&format!(
        "COMMAND curl -4 -sS {IP_CHECK_HOST} (direct baseline)"
    ))?;
    let direct_ip = diagnose_curl(log, IP_CHECK_HOST)?;
    log.write(&format!("DIRECT IP {direct_ip}"))?;

    let count = profile::list(root).len();
    let mut failures = Vec::new();
    let mut profile_ips = Vec::new();
    for index in 1..=count {
        log.write(&format!("COMMAND xvpn {index}"))?;
        if let Err(error) = use_profile(root, index as u16) {
            failures.push(format!("profile {index}: {error}"));
            continue;
        }
        log.write(&format!(
            "COMMAND curl -4 -sS {IP_CHECK_HOST} (profile {index})"
        ))?;
        match diagnose_profile_curl(log, &direct_ip, index) {
            Ok(ip) if ip == direct_ip => {
                failures.push(format!(
                    "profile {index}: returned direct IP {ip}, traffic is not using this profile"
                ));
            }
            Ok(ip) if profile_ips.iter().any(|(_, seen)| seen == &ip) => {
                let other = profile_ips
                    .iter()
                    .find(|(_, seen)| seen == &ip)
                    .map(|(other, _)| *other)
                    .unwrap_or(0);
                failures.push(format!(
                    "profile {index}: returned the same IP {ip} as profile {other}"
                ));
                profile_ips.push((index, ip));
            }
            Ok(ip) => profile_ips.push((index, ip)),
            Err(error) => failures.push(format!("profile {index}: {error}")),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn diagnostic_profile_preflight(root: &Path, log: &DiagnosticLog, yes: bool) -> Result<()> {
    println!("\nPROFILE PREFLIGHT");
    println!("Run curl through each profile before automated checks.");
    println!();

    let count = profile::list(root).len();
    let executable = std::env::current_exe().map_err(|e| format!("xvpn path: {e}"))?;
    let mut results = Vec::new();
    for index in 0..=count {
        let label = if index == 0 {
            "direct".to_string()
        } else {
            format!("profile {index}")
        };
        log.write(&format!(
            "PREFLIGHT xvpn {index} curl -4 --retry 3 {IFCONFIG_CHECK_HOST}"
        ))?;
        let output = Command::new(&executable)
            .args([
                index.to_string(),
                "curl".into(),
                "-4".into(),
                "--retry".into(),
                "3".into(),
                "--retry-all-errors".into(),
                IFCONFIG_CHECK_HOST.into(),
            ])
            .output()
            .map_err(|e| format!("preflight {label}: {e}"))?;
        let text = compact_diagnostic_output(&String::from_utf8_lossy(&output.stdout));
        let mut ip = extract_ip(&text).unwrap_or("unavailable").to_string();
        if output.status.success() && ip.parse::<std::net::Ipv6Addr>().is_ok() {
            log.write(&format!(
                "PREFLIGHT IPv6 {label} primary={ip}; requesting IPv4 fallback"
            ))?;
            let fallback = ipv4_fallback_url(IFCONFIG_CHECK_HOST);
            log.write(&format!(
                "PREFLIGHT xvpn {index} curl -4 --retry 3 {fallback} (IPv4 fallback)"
            ))?;
            let fallback_output = Command::new(&executable)
                .args([
                    index.to_string(),
                    "curl".into(),
                    "-4".into(),
                    "--retry".into(),
                    "3".into(),
                    "--retry-all-errors".into(),
                    fallback.into(),
                ])
                .output()
                .map_err(|e| format!("preflight {label} fallback: {e}"))?;
            let fallback_text =
                compact_diagnostic_output(&String::from_utf8_lossy(&fallback_output.stdout));
            match extract_ip(&fallback_text)
                .and_then(|value| value.parse::<std::net::Ipv4Addr>().ok())
            {
                Some(fallback_ip) if fallback_output.status.success() => {
                    ip = fallback_ip.to_string();
                }
                _ => {
                    let stderr = compact_diagnostic_output(&String::from_utf8_lossy(
                        &fallback_output.stderr,
                    ));
                    log.write(&format!(
                        "PREFLIGHT FALLBACK FAILURE {label} status={} stderr={stderr:?}",
                        fallback_output.status
                    ))?;
                    ip = "unavailable".to_string();
                }
            }
        }
        log.write(&format!("PREFLIGHT RESULT {label} ip={ip}"))?;
        if !output.status.success() {
            log.write(&format!("PREFLIGHT FAILURE {label} command failed"))?;
            let stderr = compact_diagnostic_output(&String::from_utf8_lossy(&output.stderr));
            if !stderr.is_empty() {
                log.write(&format!("PREFLIGHT STDERR {label} {stderr}"))?;
            }
        }
        results.push((label, ip));
    }
    for (label, ip) in &results {
        println!("  {:<12} {}", label, ip);
    }
    if yes {
        println!("  Continuing automatically (--yes).\n");
        return Ok(());
    }
    print!("\nAre these IPs correct? [Y/n] ");
    io::stdout()
        .flush()
        .map_err(|e| format!("flushing preflight prompt: {e}"))?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|e| format!("reading preflight confirmation: {e}"))?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "n" | "no") {
        return Err("diagnostic cancelled during profile preflight".into());
    }
    println!();
    Ok(())
}

fn extract_ip(output: &str) -> Option<&str> {
    output
        .split_whitespace()
        .find(|value| value.parse::<IpAddr>().is_ok())
}

fn diagnose_profile_curl(log: &DiagnosticLog, direct_ip: &str, index: usize) -> Result<String> {
    for attempt in 0..5 {
        let ip = diagnose_curl(log, IP_CHECK_HOST)?;
        if ip != direct_ip || attempt == 4 {
            return Ok(ip);
        }
        std::thread::sleep(Duration::from_secs(1));
        log.write(&format!(
            "RETRY profile {index}: still using direct IP {direct_ip}"
        ))?;
    }
    unreachable!()
}

fn diagnostic_profile_commands(root: &Path, log: &DiagnosticLog) -> Result<()> {
    let count = profile::list(root).len();
    let mut failures = Vec::new();
    for index in 0..=count {
        log.write("COMMAND xvpn off")?;
        if let Err(error) = set_mode(root, Mode::Off) {
            failures.push(format!("profile {index} start: {error}"));
            continue;
        }
        for host in [IP_CHECK_HOST, IFCONFIG_CHECK_HOST] {
            log.write(&format!("COMMAND xvpn {index} curl -4 {host}"))?;
            if let Err(error) = run_app(
                root,
                index as u16,
                &[String::from("curl"), String::from("-4"), host.into()],
                false,
            ) {
                failures.push(format!("profile {index} {host}: {error}"));
            }
        }
        log.write("COMMAND xvpn off")?;
        if let Err(error) = set_mode(root, Mode::Off) {
            failures.push(format!("profile {index} cleanup: {error}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn diagnostic_modes(root: &Path, log: &DiagnosticLog) -> Result<()> {
    for (name, mode) in [
        ("off", Mode::Off),
        ("default", Mode::Default),
        ("on", Mode::On),
    ] {
        log.write(&format!("COMMAND xvpn {name}"))?;
        set_mode(root, mode)?;
        check_curls(log)?;
        log.write("COMMAND xvpn off")?;
        set_mode(root, Mode::Off)?;
    }
    Ok(())
}

fn diagnostic_firefox(root: &Path, log: &DiagnosticLog) -> Result<()> {
    log.write("COMMAND xvpn off")?;
    set_mode(root, Mode::Off)?;
    let path = root.join(config::DEFAULT);
    let had_app = network::listed_apps(&path)?
        .iter()
        .any(|app| app == "Firefox");
    let mut added = false;
    let result = (|| {
        log.write("COMMAND xvpn apps set Firefox")?;
        add_app(root, "Firefox")?;
        added = true;
        log.write("COMMAND xvpn default")?;
        set_mode(root, Mode::Default)?;
        log.write("COMMAND open -a Firefox https://ifconfig.me")?;
        command_ok("open", &["-a", "Firefox", "https://ifconfig.me"])?;
        println!("Check the IP displayed in Firefox, then press Enter.");
        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|e| format!("reading Firefox confirmation: {e}"))?;
        Ok(())
    })();
    let mut cleanup_errors = Vec::new();
    if added && !had_app {
        cleanup_log(log, "COMMAND xvpn apps remove Firefox", &mut cleanup_errors);
        if let Err(error) = remove_app(root, "Firefox") {
            cleanup_errors.push(error);
        }
    }
    cleanup_log(log, "COMMAND xvpn default", &mut cleanup_errors);
    if let Err(error) = set_mode(root, Mode::Default) {
        cleanup_errors.push(error);
    }
    cleanup_log(log, "COMMAND xvpn off", &mut cleanup_errors);
    if let Err(error) = set_mode(root, Mode::Off) {
        cleanup_errors.push(error);
    }
    match (result, cleanup_errors.is_empty()) {
        (Err(error), _) => Err(error),
        (Ok(()), true) => Ok(()),
        (Ok(()), false) => Err(format!("Firefox cleanup failed: {cleanup_errors:?}")),
    }
}

fn check_curls(log: &DiagnosticLog) -> Result<(String, String)> {
    let ip = diagnose_curl(log, IP_CHECK_HOST)?;
    let site_ip = diagnose_curl(log, IFCONFIG_CHECK_HOST)?;
    Ok((ip, site_ip))
}

fn diagnose_curl(log: &DiagnosticLog, host: &str) -> Result<String> {
    let command = format!("curl -4 --retry 3 --retry-all-errors -sS {host}");
    log.write(&format!("COMMAND {command}"))?;
    let output = Command::new("curl")
        .args(["-4", "--retry", "3", "--retry-all-errors", "-sS", host])
        .output()
        .map_err(|e| format!("{command}: {e}"))?;
    let stdout = compact_diagnostic_output(&String::from_utf8_lossy(&output.stdout));
    let stderr = compact_diagnostic_output(&String::from_utf8_lossy(&output.stderr));
    log.write(&format!(
        "OUTPUT command={command:?} status={} stdout={stdout:?} stderr={stderr:?}",
        output.status
    ))?;
    if !stdout.is_empty() {
        log.write(&format!("STDOUT {stdout}"))?;
    }
    if !stderr.is_empty() {
        log.write(&format!("STDERR {stderr}"))?;
    }
    if !output.status.success() {
        return Err(format!("{command} failed"));
    }
    if stdout.is_empty() {
        return Err(format!("{command} returned no IP"));
    }
    match extract_ip(&stdout) {
        Some(value) if value.parse::<std::net::Ipv4Addr>().is_ok() => Ok(value.to_string()),
        Some(value) if value.parse::<std::net::Ipv6Addr>().is_ok() => {
            log.write(&format!(
                "FALLBACK IPv6 primary={value} host={host} using IPv4 endpoint"
            ))?;
            fallback_ipv4(log, host)
        }
        _ => Err(format!("{command} returned no IP")),
    }
}

fn fallback_ipv4(log: &DiagnosticLog, host: &str) -> Result<String> {
    let fallback = ipv4_fallback_url(host);
    let command = format!("curl -4 --retry 3 --retry-all-errors -sS {fallback}");
    log.write(&format!("COMMAND {command} (IPv4 fallback for {host})"))?;
    let output = Command::new("curl")
        .args(["-4", "--retry", "3", "--retry-all-errors", "-sS", fallback])
        .output()
        .map_err(|e| format!("{command}: {e}"))?;
    let stdout = compact_diagnostic_output(&String::from_utf8_lossy(&output.stdout));
    let stderr = compact_diagnostic_output(&String::from_utf8_lossy(&output.stderr));
    log.write(&format!(
        "OUTPUT command={command:?} status={} stdout={stdout:?} stderr={stderr:?}",
        output.status
    ))?;
    if !output.status.success() {
        return Err(format!("{command} failed"));
    }
    match extract_ip(&stdout).and_then(|value| value.parse::<std::net::Ipv4Addr>().ok()) {
        Some(value) => Ok(value.to_string()),
        None => Err(format!("{command} returned no IPv4 address")),
    }
}

fn ipv4_fallback_url(host: &str) -> &'static str {
    if host == IFCONFIG_CHECK_HOST {
        IPV4_FALLBACK_IFCONFIG_URL
    } else {
        IPV4_FALLBACK_IP_URL
    }
}

fn compact_diagnostic_output(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_diagnostic_display(value: &str) -> String {
    let max_chars = DIAGNOSTIC_DISPLAY_WIDTH.saturating_sub(2);
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

#[cfg(test)]
mod diagnose_tests {
    use super::*;

    #[test]
    fn named_tests_keep_execution_order() {
        let names: Vec<_> = DIAGNOSTIC_TESTS
            .iter()
            .map(|(name, _, _, _)| *name)
            .collect();
        assert_eq!(
            names,
            vec![
                "direct",
                "sites",
                "profiles",
                "profile-commands",
                "modes",
                "firefox"
            ]
        );
    }

    #[test]
    fn named_tests_reject_unknown_names() {
        assert!(diagnostic_index("not-a-test").is_err());
        assert!(diagnostic_index("all").is_err());
    }

    #[test]
    fn menu_labels_align_command_names() {
        let labels = diagnostic_menu_labels();
        let dash_positions: Vec<_> = labels
            .iter()
            .map(|label| label.find(" - ").unwrap())
            .collect();
        assert!(dash_positions
            .windows(2)
            .all(|positions| positions[0] == positions[1]));
    }

    #[test]
    fn diagnostic_output_is_single_line_and_bounded() {
        let output = compact_diagnostic_output(" first\n second\tthird ");
        assert_eq!(output, "first second third");
        let long = "x".repeat(500);
        assert_eq!(compact_diagnostic_output(&long), long);

        let displayed = truncate_diagnostic_display(&long);
        assert_eq!(displayed.chars().count(), DIAGNOSTIC_DISPLAY_WIDTH - 1);
        assert!(displayed.ends_with('…'));
    }

    #[test]
    fn preflight_accepts_ipv6_results_from_a_successful_proxy_command() {
        assert_eq!(
            extract_ip("2a09:bac5:4046:1e0a::2fe:a3"),
            Some("2a09:bac5:4046:1e0a::2fe:a3")
        );
    }

    #[test]
    fn legacy_probe_fallbacks_preserve_their_endpoint_identity() {
        assert_eq!(ipv4_fallback_url(IP_CHECK_HOST), IPV4_FALLBACK_IP_URL);
        assert_eq!(
            ipv4_fallback_url(IFCONFIG_CHECK_HOST),
            IPV4_FALLBACK_IFCONFIG_URL
        );
    }
}

pub fn repair(root: &Path) -> Result<()> {
    if let Err(e) = ensure_dependencies() {
        out::warn(&e);
        out::info("run ./install.sh to install missing dependencies");
        return Ok(());
    }
    profile::migrate_legacy(root)?;
    refresh_sidecars(root)?;
    out::status("✓", "configs repaired. the supervisor will reload");
    Ok(())
}

pub fn reset(root: &Path) -> Result<()> {
    if let Err(e) = ensure_dependencies() {
        out::warn(&e);
        out::info("run ./install.sh to install missing dependencies");
        return Ok(());
    }
    profile::migrate_legacy(root)?;

    out::section("removing configs:");
    for name in [config::XRAY, config::CONF, config::ACTIVE, config::ORDER] {
        match fs::remove_file(root.join(name)) {
            Ok(()) => out::status("✓", &format!("deleted {name}")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                out::status("→", &format!("{name}: already absent"))
            }
            Err(e) => out::status("✗", &format!("could not delete {name}: {e}")),
        }
    }
    if config::profiles_dir(root).is_dir() {
        match fs::remove_dir_all(config::profiles_dir(root)) {
            Ok(()) => out::status("✓", &format!("deleted {}", config::PROFILES)),
            Err(e) => out::status("✗", &format!("could not delete {}: {e}", config::PROFILES)),
        }
    }

    let ingress = ingress();
    let mut failures: Vec<Error> = Vec::new();
    out::section("recreating configs:");
    for (name, scope) in sidecars() {
        let path = root.join(name);
        if let Err(e) = config::write_json(&path, &config::singbox_config(scope, ingress)) {
            failures.push(format!("{name}: {e}"));
            continue;
        }
        match config::check_singbox(&path) {
            Ok(()) => out::status("✓", &format!("reset {name} (sing-box check OK)")),
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    }

    for (name, content) in [
        (config::CONF, network::CONF_DEFAULT),
        (config::MODE, "default\n"),
    ] {
        match fs::write(root.join(name), content) {
            Ok(()) => out::status("✓", &format!("reset {name}")),
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    }

    if !failures.is_empty() {
        out::error("reset finished with errors:");
        for f in &failures {
            out::error(f);
        }
        return Err(String::new());
    }
    out::status("✓", "defaults restored: mode=default, all profiles removed");
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
        let mut cfg = config::read_json(&profile.path)?;
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
    config::write_json(&tmp_path, &cfg)?;

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
    let _ = xray.wait();
    let _ = fs::remove_file(&tmp_path);
    let _ = fs::remove_file(&log_path);

    if status.success() {
        Ok(())
    } else {
        Err(format!("{app} exited with {status}"))
    }
}

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

fn command_ok(program: &str, args: &[&str]) -> Result<()> {
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
