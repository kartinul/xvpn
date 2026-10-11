use crate::cli::{AppsAction, SitesAction};

pub fn validate_apps_args(
    action: AppsAction,
    app: Option<String>,
) -> Result<(AppsAction, Option<String>), String> {
    match (&action, &app) {
        (AppsAction::Show, None) => Ok((action, None)),
        (AppsAction::Show, Some(_)) => Err("`apps show` takes no app name".to_string()),
        (_, None) => Err(format!(
            "`apps {}` needs an app name (e.g. `xvpn apps {} Telegram`)",
            match action {
                AppsAction::Show => "show",
                AppsAction::Set => "set",
                AppsAction::Remove => "remove",
            },
            match action {
                AppsAction::Show => "show",
                AppsAction::Set => "set",
                AppsAction::Remove => "remove",
            }
        )),
        _ => Ok((action, app)),
    }
}

pub fn validate_sites_args(
    action: SitesAction,
    site: Option<String>,
) -> Result<(SitesAction, Option<String>), String> {
    match (&action, &site) {
        (SitesAction::Show, None) => Ok((action, None)),
        (SitesAction::Show, Some(_)) => Err("`sites show` takes no domain".to_string()),
        (_, None) => Err(format!(
            "`sites {}` needs a domain (e.g. `xvpn sites {} netflix.com`)",
            match action {
                SitesAction::Show => "show",
                SitesAction::Set => "set",
                SitesAction::Remove => "remove",
            },
            match action {
                SitesAction::Show => "show",
                SitesAction::Set => "set",
                SitesAction::Remove => "remove",
            }
        )),
        _ => Ok((action, site)),
    }
}