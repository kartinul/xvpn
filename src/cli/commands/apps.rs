use std::fs;
use std::path::Path;

use crate::cli::commands::utils::network_config;
use crate::cli::AppsAction;
use crate::platform::network;
use crate::utils::output as out;
use crate::utils::validate_apps_args;
use crate::vpn::config::{self};
use crate::Result;

pub fn apps(root: &Path, action: AppsAction, app: Option<String>) -> Result<()> {
    let (action, app) = validate_apps_args(action, app)?;
    match (action, app) {
        (AppsAction::Show, _) => list_apps(root),
        (AppsAction::Add, Some(app)) => add_app(root, &app),
        (AppsAction::Remove, Some(app)) => remove_app(root, &app),
        _ => Err("invalid apps arguments".into()),
    }
}

pub fn list_apps(root: &Path) -> Result<()> {
    let path = network::network_rules(root)?;
    let apps = if path.is_file() {
        network::listed_apps(&path)?
    } else {
        Vec::new()
    };
    out::section(&format!("apps on network {}:", network::network_id()?));
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
    let mut shown = network::normalize_app(app)?;
    if add && network::os() == network::Os::MacOs {
        match installed_name(&shown) {
            Some(name) => shown = name,
            None => out::warn(&format!(
                "{shown}.app is not in any Applications folder; the rule matches nothing until it is"
            )),
        }
    }
    announce_edit(
        network::edit_app(&network_config(root)?, add, &shown),
        add,
        &shown,
    )
}

// rules match paths case-sensitively but the disk doesn't, so use the name as it is on disk
fn installed_name(name: &str) -> Option<String> {
    let home = config::home_dir().join("Applications");
    let files = [
        Path::new("/Applications"),
        Path::new("/Applications/Utilities"),
        Path::new("/System/Applications"),
        Path::new("/System/Applications/Utilities"),
        home.as_path(),
    ]
    .into_iter()
    .flat_map(|dir| fs::read_dir(dir).into_iter().flatten().flatten())
    .filter_map(|entry| entry.file_name().into_string().ok());
    match_app(files, name)
}

fn match_app(files: impl Iterator<Item = String>, name: &str) -> Option<String> {
    let bundle = format!("{name}.app");
    files
        .filter(|file| file.eq_ignore_ascii_case(&bundle))
        .map(|file| file[..name.len()].to_string())
        .next()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_name_takes_the_casing_on_disk() {
        let files = || ["Notes.app", "WhatsApp.app"].map(String::from).into_iter();
        assert_eq!(match_app(files(), "whatsapp").as_deref(), Some("WhatsApp"));
        assert_eq!(match_app(files(), "What"), None);
    }
}
