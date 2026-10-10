use std::path::Path;

use crate::cli::SitesAction;
use crate::platform::network;
use crate::utils::output as out;
use crate::utils::validate_sites_args;
use crate::vpn::config::{self};
use crate::Result;

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