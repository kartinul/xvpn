use clap::{Parser, Subcommand, ValueEnum};

use crate::{utils, vpn};

pub mod commands;

#[derive(Parser)]
#[command(name = "xvpn", version, about = "xray VPN manager")]
pub struct Cli {
    /// Show xvpn status messages while running an isolated command
    #[arg(short = 'v', long, global = true)]
    pub verbose: bool,

    /// Index into the profile list (0 = off, 1 = first profile, etc.)
    #[arg(value_parser, default_value_t = 0)]
    pub index: u16,

    /// Command to run with its arguments (everything after the index)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub cmd: Vec<String>,

    #[command(subcommand)]
    pub command: Option<Command>,

    /// Run the supervisor in the foreground (used by launchd/systemd)
    #[arg(long, hide = true)]
    pub supervise: bool,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Route traffic: on | default | off
    Set {
        #[arg(value_enum)]
        mode: Mode,
    },
    /// Manage the apps routed in default mode
    Apps {
        #[arg(value_enum, default_value_t = AppsAction::Show)]
        action: AppsAction,
        /// App bundle name, required for set and remove
        app: Option<String>,
    },
    /// Manage the sites routed in default mode
    Sites {
        #[arg(value_enum, default_value_t = SitesAction::Show)]
        action: SitesAction,
        /// Domain, required for set and remove. Covers subdomains too.
        site: Option<String>,
    },
    /// Manage named VPN profiles
    #[command(subcommand)]
    Profile(ProfileCmd),

    /// Use selective routing on the network you are on now
    Use,

    /// Alias for `set on`
    #[command(hide = true)]
    On,
    /// Alias for `set default`
    #[command(hide = true)]
    Default,
    /// Alias for `set off`
    #[command(hide = true)]
    Off,
    /// Alias for bare `xvpn`
    #[command(hide = true)]
    Status,
    /// Alias for `apps set`
    #[command(hide = true)]
    Add { app: String },
    /// Alias for `apps remove`
    #[command(hide = true)]
    Remove { app: String },
    /// Run the supervisor in the foreground
    #[command(hide = true)]
    Supervise,

    /// Curl ifconfig.me through the running xray proxy
    Check,
    /// Tail the supervisor and proxy logs
    Logs,
    /// Rewrite sing-box configs onto the current schema
    Repair,
    /// Delete generated configs and restore stock defaults
    Reset,
    /// Interactive diagnostics; pass a test name to run it directly
    Diagnose {
        /// Named test to run without the interactive menu (`all` runs all tests)
        test: Option<String>,
        /// Skip the profile preflight confirmation
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Manage the supervisor agent
    #[command(hide = true)]
    Agent {
        #[command(subcommand)]
        cmd: Option<AgentCmd>,
    },
    /// Restart the user supervisor agent
    #[command(hide = true)]
    Restart,
}

#[derive(Subcommand, Debug)]
pub enum AgentCmd {
    /// Install and start the user agent
    Install,
    /// Stop the supervisor and proxies
    Stop,
    /// Restart the supervisor
    Restart,
}

#[derive(ValueEnum, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Route this machine's traffic through the VPN
    On,
    /// Route only listed apps and tools through the VPN
    Default,
    /// Route nothing
    Off,
}

impl Mode {
    /// The literal written to the `mode` file, which the supervisor reads.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::On => "on",
            Mode::Default => "default",
            Mode::Off => "off",
        }
    }
}

#[derive(ValueEnum, Clone, Copy, PartialEq, Eq, Debug)]
pub enum AppsAction {
    /// List the routed apps
    Show,
    /// Route an app through the VPN
    #[value(alias = "add")]
    Set,
    /// Stop routing an app
    #[value(alias = "rm", alias = "delete")]
    Remove,
}

#[derive(ValueEnum, Clone, Copy, PartialEq, Eq, Debug)]
pub enum SitesAction {
    /// List the routed sites
    Show,
    /// Route a domain through the VPN
    #[value(alias = "add")]
    Set,
    /// Stop routing a domain
    #[value(alias = "rm", alias = "delete")]
    Remove,
}

#[derive(Subcommand, Debug)]
pub enum ProfileCmd {
    /// Store a vless:// link under a name
    Add {
        name: String,
        /// vless:// link (prompts if omitted)
        link: Option<String>,
        /// Overwrite an existing profile
        #[arg(long)]
        force: bool,
    },
    /// List stored profiles
    List,
    /// Switch to a profile
    Use {
        /// Profile name, or its position from `profile list`
        name: String,
    },
    /// Delete a profile
    Remove {
        /// Profile name, or its position from `profile list`
        name: String,
    },
    /// Print a profile's config
    Show {
        /// Name or position; defaults to the active profile
        name: Option<String>,
    },
}

pub fn dispatch(cli: Cli) {
    let root = utils::root();

    if cli.supervise {
        commands::help::run(vpn::supervisor::supervise(&root));
        return;
    }

    let result = match cli.command {
        None => {
            if !cli.cmd.is_empty() {
                commands::utils::run_app(&root, cli.index, &cli.cmd, cli.verbose)
            } else if cli.index > 0 {
                commands::profile::use_profile(&root, cli.index)
            } else {
                commands::profile::use_profile(&root, 0)
            }
        }
        Some(Command::Set { mode }) => commands::mode::set_mode(&root, mode),
        Some(Command::On) => commands::mode::set_mode(&root, Mode::On),
        Some(Command::Default) => commands::mode::set_mode(&root, Mode::Default),
        Some(Command::Off) => commands::mode::set_mode(&root, Mode::Off),
        Some(Command::Status) => commands::mode::status(&root),
        Some(Command::Apps { action, app }) => commands::apps::apps(&root, action, app),
        Some(Command::Sites { action, site }) => commands::sites::sites(&root, action, site),
        Some(Command::Add { app }) => commands::apps::add_app(&root, &app),
        Some(Command::Remove { app }) => commands::apps::remove_app(&root, &app),
        Some(Command::Profile(cmd)) => commands::profile::profile(&root, cmd),
        Some(Command::Use) => commands::network::use_network(&root, true, None),
        Some(Command::Supervise) => {
            commands::help::run(vpn::supervisor::supervise(&root));
            return;
        }

        Some(Command::Check) => commands::network::check_vpn(&root),
        Some(Command::Logs) => commands::logs::logs(),
        Some(Command::Repair) => commands::repair::repair(&root),
        Some(Command::Reset) => commands::repair::reset(&root),
        Some(Command::Diagnose { test, yes }) => commands::diagnose::diagnose(&root, test.as_deref(), yes),
        Some(Command::Agent { cmd }) => {
            let cmd = cmd.unwrap_or(AgentCmd::Install);
            commands::agent::agent(cmd, &root)
        }
        Some(Command::Restart) => commands::agent::agent(AgentCmd::Restart, &root),
    };

    commands::help::run(result);
}
