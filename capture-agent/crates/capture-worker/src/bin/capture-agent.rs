use std::{
    path::PathBuf,
    process::Command as ProcessCommand,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use capture_agent::{
    runner::{CaptureOptions, run_capture},
    source::CaptureSource,
    test_pattern::TestPatternSource,
};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "capture-agent",
    version,
    about = "Passive window capture agent"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check capture and video encoding prerequisites.
    Doctor,
    /// Capture one source into H.264/MKV video segments.
    Run {
        #[arg(long, value_enum)]
        source: SourceArg,
        #[arg(long)]
        window_id: Option<String>,
        /// Exact job output directory. Its final component is the default job id.
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        job_id: Option<String>,
        #[arg(long, default_value_t = 5)]
        segment_seconds: u64,
        #[arg(long, default_value_t = 10)]
        fps: u32,
        #[arg(long, default_value_t = 2048)]
        video_bitrate_kbps: u32,
        /// Number of segments. Use 0 to continue until Ctrl+C.
        #[arg(long, default_value_t = 1)]
        segments: u64,
        #[arg(long, default_value = "gst-launch-1.0")]
        gstreamer_launch: String,
        #[arg(long, default_value_t = 1280)]
        test_width: u32,
        #[arg(long, default_value_t = 720)]
        test_height: u32,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SourceArg {
    TestPattern,
    X11,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("capture-agent: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Doctor => doctor(),
        Command::Run {
            source,
            window_id,
            output,
            job_id,
            segment_seconds,
            fps,
            video_bitrate_kbps,
            segments,
            gstreamer_launch,
            test_width,
            test_height,
        } => {
            if segment_seconds == 0 {
                bail!("--segment-seconds must be greater than zero");
            }
            let mut capture_source: Box<dyn CaptureSource> = match source {
                SourceArg::TestPattern => Box::new(TestPatternSource::new(test_width, test_height)),
                SourceArg::X11 => {
                    let window_id =
                        window_id.context("--window-id is required for --source x11")?;
                    x11_source(&window_id)?
                }
            };
            let stop_requested = Arc::new(AtomicBool::new(false));
            let signal_flag = Arc::clone(&stop_requested);
            ctrlc::set_handler(move || signal_flag.store(true, Ordering::Relaxed))
                .context("cannot install Ctrl+C handler")?;

            let job_id = job_id
                .or_else(|| {
                    output
                        .file_name()
                        .and_then(|value| value.to_str())
                        .map(str::to_string)
                })
                .context("--job-id is required when --output has no final directory name")?;
            let summary = run_capture(
                capture_source.as_mut(),
                &CaptureOptions {
                    job_id,
                    output_directory: output,
                    segment_duration: Duration::from_secs(segment_seconds),
                    fps,
                    video_bitrate_kbps,
                    max_segments: (segments != 0).then_some(segments),
                    gstreamer_launch: gstreamer_launch.into(),
                    stop_requested,
                },
            )?;
            println!(
                "{}",
                serde_json::json!({
                    "status": "completed",
                    "jobDirectory": summary.job_directory,
                    "segments": summary.segments,
                    "frames": summary.frames,
                })
            );
            Ok(())
        }
    }
}

fn doctor() -> Result<()> {
    let x11 = x11_status();
    let gstreamer = match ProcessCommand::new("gst-launch-1.0")
        .arg("--version")
        .output()
    {
        Ok(output) if output.status.success() => serde_json::json!({
            "available": true,
            "version": String::from_utf8_lossy(&output.stdout).lines().next(),
        }),
        Ok(output) => serde_json::json!({
            "available": false,
            "error": format!("gst-launch-1.0 exited with {}", output.status),
        }),
        Err(error) => serde_json::json!({"available": false, "error": error.to_string()}),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "platform": std::env::consts::OS,
            "architecture": std::env::consts::ARCH,
            "sessionType": std::env::var("XDG_SESSION_TYPE").ok(),
            "display": std::env::var("DISPLAY").ok(),
            "videoEncoding": {
                "gstreamer": gstreamer,
                "container": "matroska",
                "codec": "h264",
                "encoder": "x264enc"
            },
            "backends": {
                "testPattern": {"available": true},
                "x11": x11,
                "waylandPortal": {
                    "available": false,
                    "reason": "not implemented in version 0.1.0"
                }
            }
        }))?
    );
    Ok(())
}

#[cfg(target_os = "linux")]
fn x11_source(window_id: &str) -> Result<Box<dyn CaptureSource>> {
    use capture_agent::x11::{X11WindowSource, parse_window_id};

    Ok(Box::new(X11WindowSource::connect(parse_window_id(
        window_id,
    )?)?))
}

#[cfg(not(target_os = "linux"))]
fn x11_source(_window_id: &str) -> Result<Box<dyn CaptureSource>> {
    bail!("the X11 capture backend is only available on Linux")
}

#[cfg(target_os = "linux")]
fn x11_status() -> serde_json::Value {
    match capture_agent::x11::list_windows() {
        Ok(windows) => serde_json::json!({"available": true, "windowCount": windows.len()}),
        Err(error) => serde_json::json!({"available": false, "error": error.to_string()}),
    }
}

#[cfg(not(target_os = "linux"))]
fn x11_status() -> serde_json::Value {
    serde_json::json!({
        "available": false,
        "reason": "the X11 backend is only available on Linux"
    })
}
