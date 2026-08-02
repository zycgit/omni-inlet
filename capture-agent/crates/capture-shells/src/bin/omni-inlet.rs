use std::{
    ffi::OsString,
    process::{Command, ExitStatus, Stdio},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "omni-inlet",
    version,
    about = "OmniInlet capture command entrypoint"
)]
struct Cli {
    #[command(subcommand)]
    command: CommandKind,
}

#[derive(Subcommand)]
enum CommandKind {
    /// Check both packaged sidecars.
    Doctor,
    /// Delegate arguments to capture-agent run.
    Capture {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    /// Delegate arguments to window-enumerator snapshot.
    Windows {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
}

fn main() {
    if let Err(error) = run() {
        eprintln!("omni-inlet: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        CommandKind::Doctor => {
            ensure_success(run_child("capture-agent", ["doctor"])?)?;
            ensure_success(run_child("window-enumerator", ["doctor"])?)?;
        }
        CommandKind::Capture { args } => {
            let mut child_args = vec![OsString::from("run")];
            child_args.extend(args);
            ensure_success(run_child("capture-agent", child_args)?)?;
        }
        CommandKind::Windows { args } => {
            let mut child_args = vec![OsString::from("snapshot")];
            child_args.extend(args);
            ensure_success(run_child("window-enumerator", child_args)?)?;
        }
    }
    Ok(())
}

fn run_child<I, S>(name: &str, arguments: I) -> Result<ExitStatus>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let executable = capture_shells::sibling_executable(name)?;
    Command::new(&executable)
        .args(arguments)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("cannot start {}", executable.display()))
}

fn ensure_success(status: ExitStatus) -> Result<()> {
    if status.success() {
        Ok(())
    } else {
        bail!("child process exited with {status}")
    }
}
