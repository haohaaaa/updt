use clap::{Arg, Command as ClapCommand, builder::PossibleValuesParser};

use crate::state::TARGET_IDS;

pub enum CliCommand {
    Default,
    Update(Vec<String>),
    Fish,
    Terminal {
        targets: Vec<String>,
        demo: bool,
        shell: bool,
    },
    TerminalDemo,
    TerminalWorker {
        targets: Vec<String>,
        owner_pid: u32,
    },
}

pub fn parse_cli() -> CliCommand {
    let matches = ClapCommand::new("updt")
        .version(env!("CARGO_PKG_VERSION"))
        .subcommand(
            ClapCommand::new("update")
                .about("Update selected targets")
                .arg(
                    Arg::new("targets")
                        .num_args(1..)
                        .required(true)
                        .value_delimiter(',')
                        .value_parser(PossibleValuesParser::new(TARGET_IDS))
                        .help("Targets to update, for example: npm,cargo"),
                ),
        )
        .subcommand(
            ClapCommand::new("terminal")
                .about("Check and update packages in an embedded interactive terminal")
                .arg(
                    Arg::new("demo")
                        .long("demo")
                        .action(clap::ArgAction::SetTrue)
                        .conflicts_with_all(["shell", "targets"])
                        .help("Try terminal interaction without running the update workflow"),
                )
                .arg(
                    Arg::new("shell")
                        .long("shell")
                        .action(clap::ArgAction::SetTrue)
                        .conflicts_with("targets")
                        .help("Open a shell without running the update workflow"),
                )
                .arg(
                    Arg::new("targets")
                        .num_args(0..)
                        .value_delimiter(',')
                        .value_parser(PossibleValuesParser::new(TARGET_IDS)),
                ),
        )
        .subcommand(ClapCommand::new("__terminal-demo").hide(true))
        .subcommand(
            ClapCommand::new("__terminal-worker")
                .hide(true)
                .arg(
                    Arg::new("owner-pid")
                        .long("owner-pid")
                        .required(true)
                        .value_parser(clap::value_parser!(u32).range(1..)),
                )
                .arg(
                    Arg::new("targets")
                        .num_args(0..)
                        .value_delimiter(',')
                        .value_parser(PossibleValuesParser::new(TARGET_IDS)),
                ),
        )
        .subcommand(
            ClapCommand::new("fish")
                .about("Install fish completion to ~/.config/fish/completions/updt.fish"),
        )
        .get_matches();

    match matches.subcommand() {
        Some(("update", sub)) => CliCommand::Update(
            sub.get_many::<String>("targets")
                .map(|v| v.cloned().collect())
                .unwrap_or_default(),
        ),
        Some(("fish", _)) => CliCommand::Fish,
        Some(("terminal", sub)) => CliCommand::Terminal {
            targets: sub
                .get_many::<String>("targets")
                .map(|values| values.cloned().collect())
                .unwrap_or_default(),
            demo: sub.get_flag("demo"),
            shell: sub.get_flag("shell"),
        },
        Some(("__terminal-demo", _)) => CliCommand::TerminalDemo,
        Some(("__terminal-worker", sub)) => CliCommand::TerminalWorker {
            targets: sub
                .get_many::<String>("targets")
                .map(|values| values.cloned().collect())
                .unwrap_or_default(),
            owner_pid: *sub.get_one::<u32>("owner-pid").expect("required owner PID"),
        },
        _ => CliCommand::Default,
    }
}
