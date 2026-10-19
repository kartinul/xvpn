use std::path::Path;

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
