use anyhow::Result;
use capture_protocol::{CaptureSourceInfo, CaptureSourceKind};
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
        #[arg(long, value_enum, default_value_t = SourceArg::X11)]
        source: SourceArg,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SourceArg {
    TestPattern,
    X11,
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
            let status = match list_x11_windows() {
                Ok(windows) => serde_json::json!({
                    "platform": std::env::consts::OS,
                    "x11": {"available": true, "windowCount": windows.len()}
                }),
                Err(error) => serde_json::json!({
                    "platform": std::env::consts::OS,
                    "x11": {"available": false, "error": error.to_string()}
                }),
            };
            println!("{}", serde_json::to_string_pretty(&status)?);
            Ok(())
        }
        Command::Snapshot { source, json } => list(source, json),
    }
}

fn list(source: SourceArg, json: bool) -> Result<()> {
    let sources: Vec<CaptureSourceInfo> = match source {
        SourceArg::TestPattern => vec![CaptureSourceInfo {
            kind: CaptureSourceKind::TestPattern,
            id: "test-pattern".to_string(),
            title: "Deterministic test pattern".to_string(),
            width: 1280,
            height: 720,
            visible: true,
        }],
        SourceArg::X11 => list_x11_windows()?,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&sources)?);
    } else {
        for source in sources {
            println!(
                "{}\t{}x{}\t{}\t{}",
                source.id,
                source.width,
                source.height,
                if source.visible { "visible" } else { "hidden" },
                source.title
            );
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn list_x11_windows() -> Result<Vec<CaptureSourceInfo>> {
    capture_agent::x11::list_windows()
}

#[cfg(not(target_os = "linux"))]
fn list_x11_windows() -> Result<Vec<CaptureSourceInfo>> {
    anyhow::bail!("the X11 window enumerator is only available on Linux")
}
