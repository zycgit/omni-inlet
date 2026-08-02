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
        XtaskCommand::Test => cargo(
            &root,
            &["test", "--workspace", "--exclude", "capture-runtime"],
        ),
        XtaskCommand::Build => build_runtime_and_shells(&root, &host_triple()?, false),
        XtaskCommand::Package { target } => package(&root, target),
        XtaskCommand::Demo {
            segments,
            segment_seconds,
            output,
        } => {
            build_runtime_and_shells(&root, &host_triple()?, false)?;
            let job_id = format!("demo-{}", unix_ms());
            let job_directory = output.join(&job_id);
            let executable = root.join("target").join("debug").join(if cfg!(windows) {
                "capture-agent.exe"
            } else {
                "capture-agent"
            });
            command(
                &executable,
                &[
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
    build_runtime_and_shells(root, &host, false)?;
    let executable = root.join("target").join("debug").join(if cfg!(windows) {
        "capture-agent.exe"
    } else {
        "capture-agent"
    });
    command(&executable, &["doctor"])
}

fn package(root: &Path, requested: TargetArg) -> Result<()> {
    let host = host_triple()?;
    let target = resolve_target(requested, &host);
    if target != host {
        bail!("full package for {target} must be built on that target OS; current host is {host}");
    }

    build_runtime_and_shells(root, &target, true)?;

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
    let lib_directory = app_directory.join("lib");
    fs::create_dir_all(&lib_directory)?;
    fs::create_dir_all(app_directory.join("resources"))?;
    fs::create_dir_all(app_directory.join("licenses"))?;

    let extension = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    for binary in ["omni-inlet", "capture-agent", "window-enumerator"] {
        copy_binary(
            root,
            &target,
            &format!("{binary}{extension}"),
            &bin_directory,
        )?;
    }
    copy_ffmpeg_runtime(
        root,
        &target,
        &lib_directory,
        &app_directory.join("licenses"),
    )?;

    println!("portable app created: {}", app_directory.display());
    Ok(())
}

fn build_runtime_and_shells(root: &Path, target: &str, release: bool) -> Result<()> {
    let triplet = vcpkg_triplet(target)?;
    let cargo_program = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut arguments = vec!["build", "-p", "capture-runtime", "-p", "capture-shells"];
    if release {
        arguments.push("--release");
        arguments.extend(["--target", target]);
    }
    let status = Command::new(cargo_program)
        .args(arguments)
        .env("VCPKGRS_TRIPLET", triplet)
        .current_dir(root)
        .status()
        .context("cannot build the capture runtime and command shells")?;
    if !status.success() {
        bail!("capture runtime or command shell build failed with {status}");
    }
    if !release {
        copy_ffmpeg_libraries(target, &root.join("target").join("debug"))?;
    }
    Ok(())
}

fn copy_ffmpeg_runtime(
    root: &Path,
    target: &str,
    destination: &Path,
    licenses: &Path,
) -> Result<()> {
    let triplet = vcpkg_triplet(target)?;
    let vcpkg_root = env::var_os("VCPKG_ROOT")
        .or_else(|| env::var_os("VCPKG_INSTALLATION_ROOT"))
        .map(PathBuf::from)
        .context("VCPKG_ROOT is required to package FFmpeg dynamic libraries")?;
    let runtime_name = if target.contains("windows") {
        "capture_runtime.dll"
    } else if target.contains("apple") {
        "libcapture_runtime.dylib"
    } else {
        "libcapture_runtime.so"
    };
    let runtime = root
        .join("target")
        .join(target)
        .join("release")
        .join(runtime_name);
    fs::copy(&runtime, destination.join(runtime_name))
        .with_context(|| format!("cannot package capture runtime {}", runtime.display()))?;

    copy_ffmpeg_libraries(target, destination)?;

    for (package, output) in [("ffmpeg", "FFmpeg.txt"), ("openh264", "OpenH264.txt")] {
        let copyright = vcpkg_root
            .join("installed")
            .join(triplet)
            .join("share")
            .join(package)
            .join("copyright");
        fs::copy(&copyright, licenses.join(output)).with_context(|| {
            format!("cannot package dependency license {}", copyright.display())
        })?;
    }
    Ok(())
}

fn copy_ffmpeg_libraries(target: &str, destination: &Path) -> Result<()> {
    let triplet = vcpkg_triplet(target)?;
    let vcpkg_root = env::var_os("VCPKG_ROOT")
        .or_else(|| env::var_os("VCPKG_INSTALLATION_ROOT"))
        .map(PathBuf::from)
        .context("VCPKG_ROOT is required to locate FFmpeg dynamic libraries")?;
    let installed = vcpkg_root.join("installed").join(triplet);
    let runtime_directory = if target.contains("windows") {
        installed.join("bin")
    } else {
        installed.join("lib")
    };
    ensure_directory(&runtime_directory, "vcpkg runtime directory")?;
    fs::create_dir_all(destination)?;

    let mut copied = 0_u32;
    for entry in fs::read_dir(&runtime_directory)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let is_required = [
            "avcodec",
            "avformat",
            "avutil",
            "swresample",
            "swscale",
            "openh264",
        ]
        .iter()
        .any(|component| name.contains(component));
        let is_runtime = is_required
            && if target.contains("windows") {
                name.ends_with(".dll")
            } else if target.contains("apple") {
                name.strip_suffix(".dylib")
                    .and_then(|stem| stem.rsplit_once('.'))
                    .is_some_and(|(_, version)| version.chars().all(|value| value.is_ascii_digit()))
            } else {
                name.split_once(".so.")
                    .is_some_and(|(_, version)| version.chars().all(|value| value.is_ascii_digit()))
            };
        if is_runtime && path.is_file() {
            fs::copy(&path, destination.join(name))?;
            copied += 1;
        }
    }
    if copied == 0 {
        bail!(
            "no FFmpeg runtime libraries found in {}",
            runtime_directory.display()
        );
    }

    Ok(())
}

fn ensure_directory(path: &Path, description: &str) -> Result<()> {
    if !path.is_dir() {
        bail!("{description} is missing: {}", path.display());
    }
    Ok(())
}

fn vcpkg_triplet(target: &str) -> Result<&'static str> {
    match target {
        "x86_64-pc-windows-msvc" => Ok("x64-windows-dynamic"),
        "x86_64-unknown-linux-gnu" => Ok("x64-linux-dynamic"),
        "aarch64-apple-darwin" => Ok("arm64-osx-dynamic"),
        _ => bail!("no bundled FFmpeg vcpkg triplet is defined for {target}"),
    }
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

fn command(program: &Path, arguments: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(arguments)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("cannot run {}", program.display()))?;
    if !status.success() {
        bail!("{} exited with {status}", program.display());
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
