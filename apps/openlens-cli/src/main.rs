//! Headless OpenLens CLI: queries, imports and the MCP entry point.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "openlens-cli", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the MCP server for agents. Both roots are required: the server
    /// refuses any file access outside them.
    Mcp {
        /// Directory agents may read from.
        #[arg(long, required = true)]
        automation_read_root: PathBuf,
        /// Directory agents may write to.
        #[arg(long, required = true)]
        automation_write_root: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Mcp {
            automation_read_root,
            automation_write_root,
        } => {
            for root in [&automation_read_root, &automation_write_root] {
                if !root.is_dir() {
                    eprintln!("error: {} is not a directory", root.display());
                    return ExitCode::FAILURE;
                }
            }
            // Report the gap honestly rather than pretending to serve.
            eprintln!("error: the MCP server is not implemented yet (ROADMAP.md, M7)");
            ExitCode::FAILURE
        }
    }
}
