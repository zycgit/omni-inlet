use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "cargo xtask",
    version,
    about = "Capture agent build entrypoint"
)]
struct Cli {
    #[command(subcommand)]
    command: XtaskCommand,
}

#[derive(Subcommand)]
enum XtaskCommand {
    /// Check local build and capture prerequisites.
    Doctor,
    /// Run all workspace tests.
    Test,
    /// Build all product executables.
    Build,
    /// Build the portable app directory for the current host.
    Package {
        #[arg(long, value_enum, default_value_t = TargetArg::Current)]
        target: TargetArg,
    },
    /// Run capture-agent with the deterministic test-pattern backend.
    Demo {
        #[arg(long, default_value_t = 1)]
        segments: u64,
        #[arg(long, default_value_t = 5)]
        segment_seconds: u64,
        #[arg(long, default_value = "capture-data")]
        output: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TargetArg {
    Current,
    WindowsX64,
    MacosArm64,
    MacosX64,
    LinuxX64,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("xtask: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let root = workspace_root()?;
    match cli.command {
        XtaskCommand::Doctor => doctor(&root),
        XtaskCommand::Test => cargo(&root, &["test", "--workspace"]),
        XtaskCommand::Build => cargo(&root, &["build", "-p", "capture-agent", "--bins"]),
        XtaskCommand::Package { target } => package(&root, target),
        XtaskCommand::Demo {
            segments,
            segment_seconds,
            output,
        } => {
            let job_id = format!("demo-{}", unix_ms());
            let job_directory = output.join(&job_id);
            cargo(
                &root,
                &[
                    "run",
                    "-p",
                    "capture-agent",
                    "--bin",
                    "capture-agent",
                    "--",
                    "run",
                    "--source",
                    "test-pattern",
                    "--segments",
                    &segments.to_string(),
                    "--segment-seconds",
                    &segment_seconds.to_string(),
                    "--job-id",
                    &job_id,
                    "--output",
                    job_directory
                        .to_str()
                        .context("demo output path is not valid UTF-8")?,
                ],
            )
        }
    }
}

fn doctor(root: &Path) -> Result<()> {
    let host = host_triple()?;
    println!("workspace       {}", root.display());
    println!("host            {host}");
    println!(
        "rustc           {}",
        command_output("rustc", &["--version"])?
    );
    println!(
        "cargo           {}",
        command_output("cargo", &["--version"])?
    );
    println!(
        "display         {}",
        env::var("DISPLAY").unwrap_or_else(|_| "<not set>".to_string())
    );
    println!(
        "session_type    {}",
        env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "<not set>".to_string())
    );
    println!();
    cargo(
        root,
        &[
            "run",
            "-q",
            "-p",
            "capture-agent",
            "--bin",
            "capture-agent",
            "--",
            "doctor",
        ],
    )
}

fn package(root: &Path, requested: TargetArg) -> Result<()> {
    let host = host_triple()?;
    let target = resolve_target(requested, &host);
    if target != host {
        bail!("full package for {target} must be built on that target OS; current host is {host}");
    }

    cargo(
        root,
        &[
            "build",
            "--release",
            "-p",
            "capture-agent",
            "--bins",
            "--target",
            &target,
        ],
    )?;

    let package_directory = root
        .join("dist")
        .join(env!("CARGO_PKG_VERSION"))
        .join(&target);
    if package_directory.exists() {
        fs::remove_dir_all(&package_directory).with_context(|| {
            format!(
                "cannot replace generated package directory {}",
                package_directory.display()
            )
        })?;
    }
    let app_directory = package_directory.join("app");

    let bin_directory = app_directory.join("bin");
    fs::create_dir_all(&bin_directory)?;
    fs::create_dir_all(app_directory.join("lib"))?;
    fs::create_dir_all(app_directory.join("resources"))?;
    fs::create_dir_all(app_directory.join("licenses"))?;

    let extension = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    copy_binary(
        root,
        &target,
        &format!("omni-inlet{extension}"),
        &app_directory,
    )?;
    for binary in ["capture-agent", "window-enumerator"] {
        copy_binary(
            root,
            &target,
            &format!("{binary}{extension}"),
            &bin_directory,
        )?;
    }

    println!("portable app created: {}", app_directory.display());
    Ok(())
}

fn copy_binary(root: &Path, target: &str, name: &str, destination: &Path) -> Result<()> {
    let source = root.join("target").join(target).join("release").join(name);
    if !source.is_file() {
        bail!(
            "cargo completed but the expected binary is missing: {}",
            source.display()
        );
    }
    let target = destination.join(name);
    fs::copy(&source, &target)
        .with_context(|| format!("cannot copy {} to {}", source.display(), target.display()))?;
    Ok(())
}

fn resolve_target(requested: TargetArg, host: &str) -> String {
    match requested {
        TargetArg::Current => host.to_string(),
        TargetArg::WindowsX64 => "x86_64-pc-windows-msvc".to_string(),
        TargetArg::MacosArm64 => "aarch64-apple-darwin".to_string(),
        TargetArg::MacosX64 => "x86_64-apple-darwin".to_string(),
        TargetArg::LinuxX64 => "x86_64-unknown-linux-gnu".to_string(),
    }
}

fn workspace_root() -> Result<PathBuf> {
    let current_directory = env::current_dir().context("cannot read current directory")?;
    for directory in current_directory.ancestors() {
        let manifest = directory.join("Cargo.toml");
        if manifest.is_file()
            && fs::read_to_string(&manifest)
                .with_context(|| format!("cannot read {}", manifest.display()))?
                .lines()
                .any(|line| line.trim() == "[workspace]")
        {
            return Ok(directory.to_path_buf());
        }
    }
    bail!(
        "cannot find a Cargo workspace above {}",
        current_directory.display()
    )
}

fn host_triple() -> Result<String> {
    let verbose = command_output("rustc", &["-vV"])?;
    verbose
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::to_string)
        .context("rustc -vV did not report a host triple")
}

fn cargo(root: &Path, arguments: &[&str]) -> Result<()> {
    let cargo_program = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo_program)
        .args(arguments)
        .current_dir(root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("cannot run cargo {}", arguments.join(" ")))?;
    if !status.success() {
        bail!("cargo {} failed with {status}", arguments.join(" "));
    }
    Ok(())
}

fn command_output(program: &str, arguments: &[&str]) -> Result<String> {
    let resolved_program = if program == "cargo" {
        env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
    } else {
        program.into()
    };
    let output = Command::new(resolved_program)
        .args(arguments)
        .output()
        .with_context(|| format!("cannot run {program}"))?;
    if !output.status.success() {
        bail!("{program} exited with {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
