use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use libloading::{Library, Symbol};

type RuntimeEntrypoint = unsafe extern "C" fn() -> i32;

pub fn exit_through_runtime(symbol: &[u8]) -> ! {
    match run_runtime(symbol) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("OmniInlet: {error:#}");
            std::process::exit(70);
        }
    }
}

fn run_runtime(symbol: &[u8]) -> Result<i32> {
    let executable = std::env::current_exe().context("cannot locate the launcher executable")?;
    let runtime = locate_runtime(&executable)?;
    let library = load_runtime(&runtime)?;
    let entrypoint: Symbol<RuntimeEntrypoint> = unsafe {
        library
            .get(symbol)
            .with_context(|| format!("{} has no requested entrypoint", runtime.display()))?
    };
    Ok(unsafe { entrypoint() })
}

fn locate_runtime(executable: &Path) -> Result<PathBuf> {
    let bin_directory = executable
        .parent()
        .context("launcher executable has no parent directory")?;
    let name = runtime_name();
    let candidates = [
        // Portable directory: app/bin -> app/lib.
        bin_directory.join("..").join("lib").join(name),
        // macOS bundle: Contents/MacOS/bin -> Contents/lib.
        bin_directory.join("..").join("..").join("lib").join(name),
        // Cargo development output: target/{debug,release}.
        bin_directory.join(name),
    ];
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .context("capture-runtime is missing beside this portable application")
}

#[cfg(target_os = "windows")]
fn runtime_name() -> &'static str {
    "capture_runtime.dll"
}

#[cfg(target_os = "macos")]
fn runtime_name() -> &'static str {
    "libcapture_runtime.dylib"
}

#[cfg(all(unix, not(target_os = "macos")))]
fn runtime_name() -> &'static str {
    "libcapture_runtime.so"
}

#[cfg(target_os = "windows")]
fn load_runtime(path: &Path) -> Result<Library> {
    use libloading::os::windows::{LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, Library as WindowsLibrary};

    const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x0000_0800;
    let library = unsafe {
        WindowsLibrary::load_with_flags(
            path,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    }
    .with_context(|| format!("cannot load {}", path.display()))?;
    Ok(library.into())
}

#[cfg(not(target_os = "windows"))]
fn load_runtime(path: &Path) -> Result<Library> {
    unsafe { Library::new(path) }.with_context(|| format!("cannot load {}", path.display()))
}

pub fn sibling_executable(name: &str) -> Result<PathBuf> {
    let entry = std::env::current_exe().context("cannot locate omni-inlet executable")?;
    let directory = entry
        .parent()
        .context("omni-inlet has no parent directory")?;
    let filename = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let executable = directory.join(filename);
    if !executable.is_file() {
        bail!("packaged command is missing: {}", executable.display());
    }
    Ok(executable)
}
