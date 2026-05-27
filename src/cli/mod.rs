//! CLI subcommand surface for the `gettext-mcp` binary.
//!
//! When the binary is invoked without a subcommand it behaves exactly
//! as before — starting the MCP server over stdio (and optionally the
//! web UI). When a subcommand is given the binary runs that one-shot
//! operation and exits, never starting the MCP server.
//!
//! Each subcommand is a thin shell that calls into the per-tool
//! handlers in [`crate::tools`]; no business logic lives here.

mod add_context;
mod common;
mod completions;
mod coverage;
mod header;
mod info;
mod search;
mod set_fuzzy;
mod stale;
mod validate;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;

pub use header::HeaderCommand;

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Show file summary (path, language, totals).
    Info {
        /// Path to .po/.pot file (auto-discovered if omitted).
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Show translation coverage for the file.
    Coverage {
        /// Path to .po/.pot file (auto-discovered if omitted).
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Run validation on translations. Exits 2 if errors found.
    Validate {
        /// Path to .po/.pot file (auto-discovered if omitted).
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Substring search across msgid + msgstr.
    Search {
        /// Substring pattern.
        pattern: String,
        /// Path to .po/.pot file (auto-discovered if omitted).
        #[arg(long)]
        path: Option<PathBuf>,
        /// Maximum number of results.
        #[arg(long, default_value = "50")]
        limit: usize,
    },
    /// List obsolete (`#~`) entries.
    Stale {
        /// Path to .po/.pot file (auto-discovered if omitted).
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Copy an existing entry into a new msgctxt (disambiguation).
    AddContext {
        /// Existing msgid to copy from (uses the no-context variant).
        msgid: String,
        /// New msgctxt to assign on the copy.
        msgctxt: String,
        /// Path to .po/.pot file (auto-discovered if omitted).
        #[arg(long)]
        path: Option<PathBuf>,
        /// Preview without writing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Toggle the fuzzy flag on an entry.
    SetFuzzy {
        /// Target msgid.
        msgid: String,
        /// Optional msgctxt.
        #[arg(long)]
        msgctxt: Option<String>,
        /// Clear the fuzzy flag instead of setting it.
        #[arg(long)]
        clear: bool,
        /// Path to .po/.pot file (auto-discovered if omitted).
        #[arg(long)]
        path: Option<PathBuf>,
        /// Preview without writing.
        #[arg(long)]
        dry_run: bool,
    },
    /// PO header field management (list / get / set).
    #[command(subcommand)]
    Header(HeaderCommand),
    /// Generate shell completion script.
    Completions {
        /// Target shell (bash, zsh, fish, elvish, powershell).
        shell: clap_complete::Shell,
    },
}

pub fn run(cmd: Command, json: bool) -> ExitCode {
    match cmd {
        Command::Info { path } => info::run(path, json),
        Command::Coverage { path } => coverage::run(path, json),
        Command::Validate { path } => validate::run(path, json),
        Command::Search {
            pattern,
            path,
            limit,
        } => search::run(pattern, path, limit, json),
        Command::Stale { path } => stale::run(path, json),
        Command::AddContext {
            msgid,
            msgctxt,
            path,
            dry_run,
        } => add_context::run(msgid, msgctxt, path, dry_run, json),
        Command::SetFuzzy {
            msgid,
            msgctxt,
            clear,
            path,
            dry_run,
        } => set_fuzzy::run(msgid, msgctxt, clear, path, dry_run, json),
        Command::Header(hcmd) => header::run(hcmd, json),
        Command::Completions { shell } => completions::run(shell),
    }
}
