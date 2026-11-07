use clap::{Arg, Command as ClapCommand, builder::PossibleValuesParser};
use clap_complete::Shell;

use crate::targets::TARGET_IDS;

mod definition;

pub use definition::build_cli;

pub enum CliCommand {
    Default,
    Update(Vec<String>),
    Fish,
    Completions(Shell),
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
    // Internal execution entry points are deliberately absent from completions.
    let matches = build_cli()
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
        .get_matches();

    match matches.subcommand() {
        Some(("update", sub)) => CliCommand::Update(
            sub.get_many::<String>("targets")
                .map(|v| v.cloned().collect())
                .unwrap_or_default(),
        ),
        Some(("fish", _)) => CliCommand::Fish,
        Some(("completions", sub)) => {
            CliCommand::Completions(*sub.get_one::<Shell>("shell").expect("required shell"))
        }
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
