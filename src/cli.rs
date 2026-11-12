use clap_complete::Shell;

mod definition;

pub use definition::build_cli;

pub enum CliCommand {
    Default,
    Update(Vec<String>),
    Fish,
    Completions(Shell),
}

pub fn parse_cli() -> CliCommand {
    let matches = build_cli().get_matches();

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
        _ => CliCommand::Default,
    }
}
