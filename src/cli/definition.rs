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
