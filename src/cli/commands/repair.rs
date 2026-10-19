use std::fs;
use std::io::{self};
use std::path::Path;

use crate::cli::commands::utils::{ensure_dependencies, ingress, refresh_sidecars, sidecars};
use crate::platform::network;
use crate::utils::output as out;
use crate::utils::write_json;
use crate::vpn::config::{self};
use crate::vpn::profile;
use crate::{Error, Result};

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
    let prompt = format!(
        "delete every profile and generated config in {}?",
        root.display()
    );
    let confirmed = dialoguer::Confirm::new()
        .with_prompt(prompt)
        .default(false)
        .interact()
        .unwrap_or(false);
    if !confirmed {
        return Err("reset cancelled, nothing deleted".into());
    }

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
        if let Err(e) = write_json(&path, &config::singbox_config(scope, ingress)) {
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
