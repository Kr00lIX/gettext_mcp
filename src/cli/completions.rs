//! `completions` subcommand — emit shell completion scripts.

use std::process::ExitCode;

use clap::CommandFactory;
use clap_complete::Shell;

use super::common::EXIT_OK;

pub fn run(shell: Shell) -> ExitCode {
    let mut cmd = crate::Cli::command();
    clap_complete::generate(shell, &mut cmd, "gettext-mcp", &mut std::io::stdout());
    ExitCode::from(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;
    use clap_complete::{generate, Shell};

    #[test]
    fn completions_emit_for_each_shell() {
        for shell in [
            Shell::Bash,
            Shell::Zsh,
            Shell::Fish,
            Shell::PowerShell,
            Shell::Elvish,
        ] {
            let mut cmd = crate::Cli::command();
            let mut buf: Vec<u8> = Vec::new();
            generate(shell, &mut cmd, "gettext-mcp", &mut buf);
            let s = String::from_utf8(buf).expect("utf8");
            assert!(!s.is_empty(), "completions for {shell:?} were empty");
        }
    }
}
