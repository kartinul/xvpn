pub mod cli;
pub mod platform;
pub mod utils;
pub mod vpn;

pub type Error = String;

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selecting_profile_from_off_enables_full_routing() {
        assert_eq!(
            cli::commands::profile::mode_after_profile_selection("off"),
            cli::Mode::On
        );
    }

    #[test]
    fn selective_config_does_not_proxy_hardcoded_commands() {
        let config = vpn::config::singbox_config(vpn::config::Scope::Selective);
        let rules = config["route"]["rules"].as_array().expect("route rules");
        assert!(!rules.iter().any(|rule| {
            rule.get("process_name")
                .and_then(|names| names.as_array())
                .is_some_and(|names| {
                    names.iter().any(|name| {
                        matches!(
                            name.as_str(),
                            Some("curl" | "git" | "brew" | "ssh" | "node" | "python3")
                        )
                    })
                })
        }));
    }
}
