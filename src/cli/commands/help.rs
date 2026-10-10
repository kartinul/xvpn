

use crate::cli::commands::agent::agent;
use crate::cli::commands::apps::{add_app, apps, remove_app};
use crate::cli::commands::diagnose::diagnose;
use crate::cli::commands::logs::logs;
use crate::cli::commands::mode::{set_mode, status};
use crate::cli::commands::network::{check_vpn, use_network};
use crate::cli::commands::profile::{profile, use_profile};
use crate::cli::commands::repair::{reset, repair};
use crate::cli::commands::sites::sites;
use crate::cli::commands::utils::run_app;
use crate::Result;

pub fn run(result: Result<()>) {
    if let Err(e) = result {
        if !e.is_empty() {
            eprintln!("Error: {e}");
        }
        std::process::exit(1);
    }
}

/// Main CLI dispatch function - routes commands to appropriate handlers
pub fn dispatch(cli: crate::cli::Cli) {
    let root = crate::utils::root();

    if cli.supervise {
        run(crate::vpn::supervisor::supervise(&root));
        return;
    }

    let result = match cli.command {
        None => {
            if !cli.cmd.is_empty() {
                run_app(&root, cli.index, &cli.cmd, cli.verbose)
            } else if cli.index > 0 {
                use_profile(&root, cli.index)
            } else {
                use_profile(&root, 0)
            }
        }
        Some(crate::cli::Command::Set { mode }) => set_mode(&root, mode),
        Some(crate::cli::Command::On) => set_mode(&root, crate::cli::Mode::On),
        Some(crate::cli::Command::Default) => set_mode(&root, crate::cli::Mode::Default),
        Some(crate::cli::Command::Off) => set_mode(&root, crate::cli::Mode::Off),
        Some(crate::cli::Command::Status) => status(&root),
        Some(crate::cli::Command::Apps { action, app }) => apps(&root, action, app),
        Some(crate::cli::Command::Sites { action, site }) => sites(&root, action, site),
        Some(crate::cli::Command::Add { app }) => add_app(&root, &app),
        Some(crate::cli::Command::Remove { app }) => remove_app(&root, &app),
        Some(crate::cli::Command::Profile(cmd)) => profile(&root, cmd),
        Some(crate::cli::Command::Use) => use_network(&root, true, None),
        Some(crate::cli::Command::Supervise) => {
            run(crate::vpn::supervisor::supervise(&root));
            return;
        }

        Some(crate::cli::Command::Check) => check_vpn(&root),
        Some(crate::cli::Command::Logs) => logs(),
        Some(crate::cli::Command::Repair) => repair(&root),
        Some(crate::cli::Command::Reset) => reset(&root),
        Some(crate::cli::Command::Diagnose { test, yes }) => diagnose(&root, test.as_deref(), yes),
        Some(crate::cli::Command::Agent { cmd }) => {
            let cmd = cmd.unwrap_or(crate::cli::AgentCmd::Install);
            agent(cmd, &root)
        }
        Some(crate::cli::Command::Restart) => agent(crate::cli::AgentCmd::Restart, &root),
    };

    run(result);
}