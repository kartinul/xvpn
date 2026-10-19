use std::fs;
use std::io::{self, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::cli::commands::agent::command_ok;
use crate::cli::commands::apps::{add_app, remove_app};
use crate::cli::commands::network::{
    IFCONFIG_CHECK_HOST, IPV4_FALLBACK_IFCONFIG_URL, IPV4_FALLBACK_IP_URL, IP_CHECK_HOST,
};
use crate::cli::commands::profile::use_profile;
use crate::cli::commands::sites::{add_site, remove_site};
use crate::cli::commands::utils::run_app;
use crate::cli::Mode;
use crate::platform::network;
use crate::utils::output as out;
use crate::vpn::config::{self};
use crate::vpn::profile;
use crate::Result;

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap},
    Terminal,
};

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

const DIAGNOSTIC_DISPLAY_WIDTH: usize = 78;

enum TuiEvent {
    Start(usize),
    Output(usize, String),
    Complete(usize, Option<String>),
}

#[cfg(test)]
fn diagnostic_menu_labels() -> Vec<String> {
    DIAGNOSTIC_TESTS
        .iter()
        .map(|(_, command, description, _)| format!("{command:<30} - {description}"))
        .collect()
}

struct DiagnosticLog {
    path: PathBuf,
    live: Option<std::sync::mpsc::Sender<TuiEvent>>,
    task_index: Option<usize>,
}

impl DiagnosticLog {
    fn new() -> Result<Self> {
        Self::with_live(None)
    }

    fn with_live(live: Option<std::sync::mpsc::Sender<TuiEvent>>) -> Result<Self> {
        let path = config::log_file("diagnose");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("creating diagnose log directory: {e}"))?;
        }
        Ok(Self {
            path,
            live,
            task_index: None,
        })
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
        if let Some(live) = &self.live {
            let _ = live.send(TuiEvent::Output(
                self.task_index.unwrap_or_default(),
                truncate_diagnostic_display(message),
            ));
        } else {
            println!("  {}", truncate_diagnostic_display(message));
            io::stdout()
                .flush()
                .map_err(|e| format!("flushing diagnose output: {e}"))?;
        }
        Ok(())
    }
}

pub fn diagnose(root: &Path, test: Option<&str>, yes: bool) -> Result<()> {
    let log = DiagnosticLog::new()?;
    let selected = match test {
        Some("all") => (0..DIAGNOSTIC_TESTS.len()).collect(),
        Some(name) => vec![diagnostic_index(name)?],
        None => {
            let defaults: Vec<usize> = DIAGNOSTIC_TESTS
                .iter()
                .enumerate()
                .filter_map(|(index, (_, _, _, default))| (*default).then_some(index))
                .collect();
            if yes {
                if defaults
                    .iter()
                    .any(|index| DIAGNOSTIC_TESTS[*index].0 == "profiles")
                {
                    diagnostic_profile_preflight(root, &log, yes)?;
                }
                return run_diagnostics(root, defaults, yes, log);
            }
            if defaults
                .iter()
                .any(|index| DIAGNOSTIC_TESTS[*index].0 == "profiles")
            {
                diagnostic_profile_preflight(root, &log, yes)?;
            }
            return diagnose_tui(root, defaults, yes);
        }
    };
    run_diagnostics(root, selected, yes, log)
}

fn run_diagnostics(root: &Path, selected: Vec<usize>, yes: bool, log: DiagnosticLog) -> Result<()> {
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
        out::set_quiet(true);
        let result = run_diagnostic_test(root, name, &log);
        out::set_quiet(false);
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

    let cleanup = crate::cli::commands::mode::set_mode(root, Mode::Off);
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiagnosticState {
    Pending,
    Running,
    Passed,
    Failed,
}

struct TuiTask {
    selected: bool,
    state: DiagnosticState,
    output: Vec<String>,
    error: Option<String>,
}

fn diagnose_tui(root: &Path, defaults: Vec<usize>, _yes: bool) -> Result<()> {
    let mut tasks: Vec<TuiTask> = DIAGNOSTIC_TESTS
        .iter()
        .enumerate()
        .map(|(index, _)| TuiTask {
            selected: defaults.contains(&index),
            state: DiagnosticState::Pending,
            output: Vec::new(),
            error: None,
        })
        .collect();
    let mut cursor = 0usize;
    let mut expanded = false;
    let mut running = false;
    let mut completed = false;
    let mut started = false;
    let mut running_index: Option<usize> = None;
    let mut receiver: Option<std::sync::mpsc::Receiver<TuiEvent>> = None;
    let mut worker: Option<std::thread::JoinHandle<()>> = None;

    enable_raw_mode().map_err(|e| format!("enabling diagnose terminal mode: {e}"))?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(|e| format!("opening diagnose screen: {e}"))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| format!("creating diagnose UI: {e}"))?;

    let result = (|| -> Result<()> {
        loop {
            while let Some(rx) = &receiver {
                match rx.try_recv() {
                    Ok(TuiEvent::Start(index)) => {
                        started = true;
                        running_index = Some(index);
                        tasks[index].state = DiagnosticState::Running;
                    }
                    Ok(TuiEvent::Output(index, line)) => tasks[index].output.push(line),
                    Ok(TuiEvent::Complete(index, error)) => {
                        tasks[index].state = if error.is_some() {
                            DiagnosticState::Failed
                        } else {
                            DiagnosticState::Passed
                        };
                        tasks[index].error = error;
                        if running_index == Some(index) {
                            running_index = None;
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        receiver = None;
                        running = false;
                        completed = true;
                        running_index = None;
                        if let Some(handle) = worker.take() {
                            handle
                                .join()
                                .map_err(|_| "diagnose worker panicked".to_string())?;
                        }
                        break;
                    }
                }
            }

            terminal
                .draw(|frame| {
                    draw_diagnose(
                        frame,
                        &tasks,
                        DrawState {
                            cursor,
                            expanded,
                            running,
                            completed,
                            started,
                            running_index,
                        },
                    )
                })
                .map_err(|e| format!("drawing diagnose UI: {e}"))?;

            if event::poll(Duration::from_millis(80))
                .map_err(|e| format!("reading diagnose input: {e}"))?
            {
                let Event::Key(key) =
                    event::read().map_err(|e| format!("reading diagnose key: {e}"))?
                else {
                    continue;
                };
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc if !running => break,
                    KeyCode::Up if !running => cursor = cursor.saturating_sub(1),
                    KeyCode::Down if !running => {
                        cursor = (cursor + 1).min(tasks.len().saturating_sub(1))
                    }
                    KeyCode::Char(' ') if !running => {
                        tasks[cursor].selected = !tasks[cursor].selected
                    }
                    KeyCode::Left | KeyCode::Right => expanded = !expanded,
                    KeyCode::Enter if !running => {
                        let selected: Vec<_> = tasks
                            .iter()
                            .enumerate()
                            .filter_map(|(i, task)| task.selected.then_some(i))
                            .collect();
                        if selected.is_empty() {
                            continue;
                        }
                        for task in &mut tasks {
                            task.state = DiagnosticState::Pending;
                            task.output.clear();
                            task.error = None;
                        }
                        let (tx, rx) = std::sync::mpsc::channel();
                        let root = root.to_path_buf();
                        receiver = Some(rx);
                        running = true;
                        let selected_for_worker = selected.clone();
                        worker = Some(std::thread::spawn(move || {
                            let mut failures = Vec::new();
                            for index in selected_for_worker {
                                let _ = tx.send(TuiEvent::Start(index));
                                let mut log = DiagnosticLog::with_live(Some(tx.clone()))
                                    .expect("diagnose log");
                                log.task_index = Some(index);
                                out::set_quiet(true);
                                let result =
                                    run_diagnostic_test(&root, DIAGNOSTIC_TESTS[index].0, &log);
                                out::set_quiet(false);
                                let error = result.err();
                                let _ = tx.send(TuiEvent::Complete(index, error.clone()));
                                if let Some(error) = error {
                                    failures.push((index, error));
                                }
                            }
                            let cleanup = crate::cli::commands::mode::set_mode(&root, Mode::Off);
                            if let Err(error) = cleanup {
                                let _ = tx.send(TuiEvent::Complete(
                                    usize::MAX,
                                    Some(format!("final cleanup: {error}")),
                                ));
                                failures.push((usize::MAX, format!("final cleanup: {error}")));
                            }
                            let _ = failures;
                        }));
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    })();

    if let Some(handle) = worker {
        let _ = handle.join();
    }
    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();
    if result.is_ok()
        && tasks
            .iter()
            .any(|task| task.selected && task.state == DiagnosticState::Failed)
    {
        Err(String::new())
    } else {
        result
    }
}

struct DrawState {
    cursor: usize,
    expanded: bool,
    running: bool,
    completed: bool,
    started: bool,
    running_index: Option<usize>,
}

fn draw_diagnose(frame: &mut ratatui::Frame<'_>, tasks: &[TuiTask], state: DrawState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(frame.area());
    let title = Paragraph::new(if state.completed {
        "diagnose  ·  results shown  ←/→ details  q quit"
    } else if state.started {
        "diagnose  ·  running  ←/→ details  q quit"
    } else {
        "diagnose  ·  Space select  Enter run  ←/→ details  q quit"
    })
    .block(Block::default().borders(Borders::ALL).title("xvpn"));
    frame.render_widget(title, chunks[0]);
    let body = if state.expanded {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
            .split(chunks[1])
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(100), Constraint::Percentage(0)])
            .split(chunks[1])
    };
    let visible: Vec<(usize, &TuiTask)> = if state.started {
        tasks
            .iter()
            .enumerate()
            .filter(|(_, task)| task.selected)
            .collect()
    } else {
        tasks.iter().enumerate().collect()
    };
    let local_cursor = if state.started {
        visible
            .iter()
            .position(|&(i, _)| i == state.cursor)
            .unwrap_or(0)
    } else {
        state.cursor
    };
    let items: Vec<_> = visible
        .iter()
        .map(|(index, task)| {
            let marker = if task.selected { "☑" } else { "☐" };
            let status = match task.state {
                DiagnosticState::Pending => "pending",
                DiagnosticState::Running => {
                    const FRAMES: [&str; 4] = ["◐ running", "◓ running", "◑ running", "◒ running"];
                    FRAMES[(std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis()
                        / 120) as usize
                        % FRAMES.len()]
                }
                DiagnosticState::Passed => "✓ pass",
                DiagnosticState::Failed => "✗ fail",
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{marker} {:02} ", index + 1),
                    Style::default().fg(if task.selected {
                        Color::Green
                    } else {
                        Color::DarkGray
                    }),
                ),
                Span::raw(DIAGNOSTIC_TESTS[*index].0),
                Span::styled(format!("  {status}"), task_style(task.state)),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("checks"))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));
    let mut list_state = ratatui::widgets::ListState::default();
    list_state.select(Some(local_cursor));
    frame.render_stateful_widget(list, body[0], &mut list_state);
    if state.expanded {
        let focus = state
            .running_index
            .unwrap_or(state.cursor.min(tasks.len().saturating_sub(1)));
        let task = &tasks[focus];
        let lines: Vec<Line<'_>> = if task.output.is_empty() {
            vec![Line::from("No output yet.")]
        } else {
            task.output.iter().map(|line| tui_log_line(line)).collect()
        };
        let title = if let Some(index) = state.running_index {
            format!("live output · {}", DIAGNOSTIC_TESTS[index].0)
        } else {
            "live output".to_string()
        };
        let output = Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(title));
        frame.render_widget(output, body[1]);
    }
    let progress = tasks
        .iter()
        .filter(|t| matches!(t.state, DiagnosticState::Passed | DiagnosticState::Failed))
        .count();
    let total = tasks.iter().filter(|t| t.selected).count().max(1);
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(if state.running {
            Color::Yellow
        } else if state.completed {
            if tasks
                .iter()
                .any(|t| t.selected && t.state == DiagnosticState::Failed)
            {
                Color::Red
            } else {
                Color::Green
            }
        } else {
            Color::Green
        }))
        .ratio(progress as f64 / total as f64)
        .label(format!("{progress}/{total} complete"));
    frame.render_widget(gauge, chunks[2]);
}

fn tui_log_line(line: &str) -> Line<'_> {
    if let Some(rest) = line.strip_prefix("COMMAND ") {
        Line::from(vec![Span::styled(
            format!("> {rest}"),
            Style::default().fg(Color::Cyan),
        )])
    } else if let Some(rest) = line.strip_prefix("STDERR ") {
        Line::from(vec![Span::styled(
            format!("  {rest}"),
            Style::default().fg(Color::Red),
        )])
    } else if line.starts_with("DIRECT IP ") || line.starts_with("PREFLIGHT RESULT ") {
        Line::from(vec![Span::styled(
            line.to_string(),
            Style::default().fg(Color::Yellow),
        )])
    } else {
        Line::from(line.to_string())
    }
}

fn task_style(state: DiagnosticState) -> Style {
    Style::default().fg(match state {
        DiagnosticState::Pending => Color::DarkGray,
        DiagnosticState::Running => Color::Yellow,
        DiagnosticState::Passed => Color::Green,
        DiagnosticState::Failed => Color::Red,
    })
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
    let cleanup = crate::cli::commands::mode::set_mode(root, Mode::Off);
    log.write(if cleanup.is_ok() {
        "CLEANUP test=xvpn off result=PASS"
    } else {
        "CLEANUP test=xvpn off result=FAIL"
    })?;
    result.and(cleanup)
}

fn diagnostic_direct(root: &Path, log: &DiagnosticLog) -> Result<()> {
    log.write("COMMAND xvpn off")?;
    crate::cli::commands::mode::set_mode(root, Mode::Off)?;
    check_curls(log).map(|_| ())
}

fn diagnostic_sites(root: &Path, log: &DiagnosticLog) -> Result<()> {
    let direct = direct_ip(root, log)?;
    let snapshot: Vec<(PathBuf, Option<Vec<u8>>)> = [config::DEFAULT, config::CONF]
        .iter()
        .map(|name| root.join(name))
        .map(|path| (path.clone(), fs::read(&path).ok()))
        .collect();
    let result = (|| {
        let sites = network::listed_sites(&root.join(config::DEFAULT))?;
        log.write("COMMAND xvpn use")?;
        network::register(root)?;
        if sites.iter().any(|site| site == IFCONFIG_CHECK_HOST) {
            log.write(&format!("COMMAND xvpn sites remove {IFCONFIG_CHECK_HOST}"))?;
            remove_site(root, IFCONFIG_CHECK_HOST)?;
        }
        if !sites.iter().any(|site| site == IP_CHECK_HOST) {
            log.write(&format!("COMMAND xvpn sites set {IP_CHECK_HOST}"))?;
            add_site(root, IP_CHECK_HOST)?;
        }
        log.write("COMMAND xvpn default")?;
        crate::cli::commands::mode::set_mode(root, Mode::Default)?;
        let (ip_me, ifconfig) = check_curls(log)?;
        if ip_me == direct {
            return Err(format!(
                "{IP_CHECK_HOST} went direct ({ip_me}), expected the VPN"
            ));
        }
        if ifconfig != direct {
            return Err(format!(
                "{IFCONFIG_CHECK_HOST} went via {ifconfig}, expected direct {direct}"
            ));
        }
        Ok(())
    })();
    let mut cleanup_errors = Vec::new();
    cleanup_log(log, "COMMAND xvpn off", &mut cleanup_errors);
    if let Err(error) = crate::cli::commands::mode::set_mode(root, Mode::Off) {
        cleanup_errors.push(error);
    }
    for (path, bytes) in &snapshot {
        cleanup_log(
            log,
            &format!("RESTORE {}", path.display()),
            &mut cleanup_errors,
        );
        let restored = match bytes {
            Some(bytes) => fs::write(path, bytes),
            None => fs::remove_file(path),
        };
        if let Err(error) = restored {
            cleanup_errors.push(format!("restoring {}: {error}", path.display()));
        }
    }
    match (result, cleanup_errors.is_empty()) {
        (Err(error), _) => Err(error),
        (Ok(()), true) => Ok(()),
        (Ok(()), false) => Err(format!("site cleanup failed: {cleanup_errors:?}")),
    }
}

fn direct_ip(root: &Path, log: &DiagnosticLog) -> Result<String> {
    log.write("COMMAND xvpn off")?;
    crate::cli::commands::mode::set_mode(root, Mode::Off)?;
    diagnose_curl(log, IP_CHECK_HOST)
}

fn cleanup_log(log: &DiagnosticLog, message: &str, errors: &mut Vec<String>) {
    if let Err(error) = log.write(message) {
        errors.push(error);
    }
}

fn diagnostic_profiles(root: &Path, log: &DiagnosticLog) -> Result<()> {
    log.write("COMMAND xvpn off")?;
    crate::cli::commands::mode::set_mode(root, Mode::Off)?;
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
            "PREFLIGHT xvpn {index} curl -4 --max-time 15 --retry 3 --retry-max-time 40 {IFCONFIG_CHECK_HOST}"
        ))?;
        let output = Command::new(&executable)
            .args([
                index.to_string(),
                "curl".into(),
                "-4".into(),
                "--max-time".into(),
                "15".into(),
                "--retry".into(),
                "3".into(),
                "--retry-max-time".into(),
                "40".into(),
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
                "PREFLIGHT xvpn {index} curl -4 --max-time 15 --retry 3 --retry-max-time 40 {fallback} (IPv4 fallback)"
            ))?;
            let fallback_output = Command::new(&executable)
                .args([
                    index.to_string(),
                    "curl".into(),
                    "-4".into(),
                    "--max-time".into(),
                    "15".into(),
                    "--retry".into(),
                    "3".into(),
                    "--retry-max-time".into(),
                    "40".into(),
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
        if let Err(error) = crate::cli::commands::mode::set_mode(root, Mode::Off) {
            failures.push(format!("profile {index} start: {error}"));
            continue;
        }
        for host in [IP_CHECK_HOST, IFCONFIG_CHECK_HOST] {
            log.write(&format!(
                "COMMAND xvpn {index} curl -4 --max-time 15 {host}"
            ))?;
            if let Err(error) = run_app(
                root,
                index as u16,
                &["curl", "-4", "--max-time", "15", host].map(String::from),
                false,
            ) {
                failures.push(format!("profile {index} {host}: {error}"));
            }
        }
        log.write("COMMAND xvpn off")?;
        if let Err(error) = crate::cli::commands::mode::set_mode(root, Mode::Off) {
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
    let direct = direct_ip(root, log)?;
    for (name, mode) in [
        ("off", Mode::Off),
        ("default", Mode::Default),
        ("on", Mode::On),
    ] {
        log.write(&format!("COMMAND xvpn {name}"))?;
        crate::cli::commands::mode::set_mode(root, mode)?;
        let (ip_me, ifconfig) = check_curls(log)?;
        let routed = [&ip_me, &ifconfig].map(|ip| *ip != direct);
        match name {
            "off" if routed.contains(&true) => {
                return Err(format!(
                    "off still routes: {ip_me} / {ifconfig}, direct {direct}"
                ))
            }
            "on" if routed.contains(&false) => {
                return Err(format!(
                    "on leaks direct: {ip_me} / {ifconfig}, direct {direct}"
                ))
            }
            _ => {}
        }
        log.write("COMMAND xvpn off")?;
        crate::cli::commands::mode::set_mode(root, Mode::Off)?;
    }
    Ok(())
}

fn diagnostic_firefox(root: &Path, log: &DiagnosticLog) -> Result<()> {
    log.write("COMMAND xvpn off")?;
    crate::cli::commands::mode::set_mode(root, Mode::Off)?;
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
        crate::cli::commands::mode::set_mode(root, Mode::Default)?;
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
    if let Err(error) = crate::cli::commands::mode::set_mode(root, Mode::Default) {
        cleanup_errors.push(error);
    }
    cleanup_log(log, "COMMAND xvpn off", &mut cleanup_errors);
    if let Err(error) = crate::cli::commands::mode::set_mode(root, Mode::Off) {
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
    let command = format!(
        "curl -4 --max-time 15 --retry 3 --retry-max-time 40 --retry-all-errors -sS {host}"
    );
    log.write(&format!("COMMAND {command}"))?;
    let output = Command::new("curl")
        .args([
            "-4",
            "--max-time",
            "15",
            "--retry",
            "3",
            "--retry-max-time",
            "40",
            "--retry-all-errors",
            "-sS",
            host,
        ])
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
    let command = format!(
        "curl -4 --max-time 15 --retry 3 --retry-max-time 40 --retry-all-errors -sS {fallback}"
    );
    log.write(&format!("COMMAND {command} (IPv4 fallback for {host})"))?;
    let output = Command::new("curl")
        .args([
            "-4",
            "--max-time",
            "15",
            "--retry",
            "3",
            "--retry-max-time",
            "40",
            "--retry-all-errors",
            "-sS",
            fallback,
        ])
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
