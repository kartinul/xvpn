use std::io::{self, Write};
use std::path::Path;

use crate::cli::commands::mode::{read_mode, set_mode};
use crate::cli::commands::utils::refresh_sidecars;
use crate::cli::{Mode, ProfileCmd};
use crate::utils::output as out;
use crate::utils::read_json;
use crate::vpn::profile;
use crate::vpn::supervisor;
use crate::Result;

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
        supervisor::authorize_transition(root, target_mode.as_str())?;
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
            if let Some(active) = profile::active_name(root) {
                out::kv("active", &active);
            }
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
            let cfg = read_json(&profile::path_for(root, &name))?;
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
