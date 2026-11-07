use clap::{Arg, Command as ClapCommand, builder::PossibleValuesParser};
use clap_complete::Shell;

use crate::targets::TARGET_IDS;

/// Public commands shared by argument parsing and shell completion generation.
pub fn build_cli() -> ClapCommand {
    ClapCommand::new("updt")
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
        .subcommand(
            ClapCommand::new("fish")
                .about("Install fish completion to the user's fish configuration directory"),
        )
        .subcommand(
            ClapCommand::new("completions")
                .about("Generate shell completions to stdout")
                .arg(
                    Arg::new("shell")
                        .required(true)
                        .value_parser(clap::value_parser!(Shell)),
                ),
        )
}
