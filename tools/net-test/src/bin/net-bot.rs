//! `net-bot ctl [--result F] [--timeline F]`: the bot under the control of the runner (JSON lines on stdin
//! and stdout).

use std::io::{self, BufReader};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use net_test::bot::ctl::{Ctl, CtlOptions};
use net_test::bot::mission::{Mission, play};

#[derive(Parser)]
#[command(
    name = "net-bot",
    about = "Headless client of protocol v3 for the net-test stand"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Be driven by JSON lines on stdin.
    Ctl {
        /// Where `result.json` goes on `quit`.
        #[arg(long)]
        result: Option<PathBuf>,
        /// `timeline.jsonl`, every event of the bot.
        #[arg(long)]
        timeline: Option<PathBuf>,
    },
    /// Play a mission (JSON file): lobby, turns, persona; then write `result.json`.
    Play {
        #[arg(long)]
        mission: PathBuf,
        #[arg(long)]
        result: Option<PathBuf>,
        #[arg(long)]
        timeline: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Play {
            mission,
            result,
            timeline,
        } => {
            let text = match std::fs::read_to_string(&mission) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("net-bot: {}: {e}", mission.display());
                    return ExitCode::FAILURE;
                }
            };
            let m: Mission = match serde_json::from_str(&text) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("net-bot: the mission: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let out = play(&m, timeline);
            let clean = out["errors"].as_array().is_some_and(Vec::is_empty);
            let body = serde_json::to_vec_pretty(&out).unwrap_or_default();
            if let Some(path) = result
                && let Err(e) = std::fs::write(&path, &body)
            {
                eprintln!("net-bot: {}: {e}", path.display());
            }
            // The runner reads the result from the file; stdout carries it for a run by hand.
            println!("{}", String::from_utf8_lossy(&body));
            if clean {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Command::Ctl { result, timeline } => {
            let mut ctl = Ctl::new(CtlOptions { result, timeline });
            let stdin = BufReader::new(io::stdin());
            match ctl.run(stdin, &mut io::stdout()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("net-bot: {e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}
