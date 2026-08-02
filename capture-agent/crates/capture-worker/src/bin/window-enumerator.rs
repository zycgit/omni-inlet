use anyhow::Result;
use std::path::PathBuf;

use capture_protocol::{ApplicationInfo, NativeTarget, WindowCandidate};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "window-enumerator",
    version,
    about = "Window enumeration sidecar"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check whether enumeration is available.
    Doctor,
    /// Print one window snapshot.
    Snapshot {
        #[arg(long, value_enum, default_value_t = SourceArg::Native)]
        source: SourceArg,
        #[arg(long)]
        thumbnail_dir: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SourceArg {
    Native,
    TestPattern,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("window-enumerator: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Doctor => {
            let status = match capture_agent::platform::enumerate_windows(None) {
                Ok(windows) => serde_json::json!({
                    "platform": std::env::consts::OS,
                    "native": {"available": true, "windowCount": windows.len()}
                }),
                Err(error) => serde_json::json!({
                    "platform": std::env::consts::OS,
                    "native": {"available": false, "error": error.to_string()}
                }),
            };
            println!("{}", serde_json::to_string_pretty(&status)?);
            Ok(())
        }
        Command::Snapshot {
            source,
            thumbnail_dir,
            json,
        } => list(source, thumbnail_dir, json),
    }
}

fn list(source: SourceArg, thumbnail_dir: Option<PathBuf>, json: bool) -> Result<()> {
    let sources: Vec<WindowCandidate> = match source {
        SourceArg::TestPattern => vec![WindowCandidate {
            candidate_id: "test-pattern:test-pattern".to_string(),
            application: ApplicationInfo {
                group_id: "test:omni-inlet".to_string(),
                display_name: "OmniInlet 测试源".to_string(),
                process_id: Some(std::process::id()),
                icon_path: None,
            },
            title: "Deterministic test pattern".to_string(),
            visible: true,
            capturable: true,
            unavailable_reason: None,
            thumbnail_path: None,
            width: 1280,
            height: 720,
            native_target: NativeTarget {
                kind: "test-pattern".to_string(),
                value: "test-pattern".to_string(),
            },
        }],
        SourceArg::Native => capture_agent::platform::enumerate_windows(thumbnail_dir.as_deref())?,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&sources)?);
    } else {
        for source in sources {
            println!(
                "{}\t{}x{}\t{}\t{}",
                source.native_target.value,
                source.width,
                source.height,
                if source.visible { "visible" } else { "hidden" },
                source.title
            );
        }
    }
    Ok(())
}
