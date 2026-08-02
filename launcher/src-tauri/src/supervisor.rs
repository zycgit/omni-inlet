use std::{
    collections::{HashMap, HashSet},
    env,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use capture_protocol::{
    AgentLease, NativeTarget, WindowCandidate, active_agent_leases, agents_directory,
};
use chrono::Local;
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, State};

type ChildHandle = Arc<Mutex<Child>>;

#[derive(Default)]
pub struct Supervisor {
    children: Mutex<HashMap<String, ChildHandle>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartCaptureRequest {
    target: NativeTarget,
    title: String,
    output_root: String,
    fps: u32,
    segment_seconds: u64,
    bitrate_kbps: u32,
}

#[tauri::command]
pub async fn enumerate_windows(app: AppHandle) -> Result<Vec<WindowCandidate>, String> {
    ensure_capture_permission()?;
    tauri::async_runtime::spawn_blocking(move || enumerate_windows_blocking(&app))
        .await
        .map_err(error_string)?
}

fn enumerate_windows_blocking(app: &AppHandle) -> Result<Vec<WindowCandidate>, String> {
    let executable = resolve_program(app, "window-enumerator")?;
    let thumbnail_directory = runtime_directory()
        .map_err(error_string)?
        .join("thumbnails");
    std::fs::create_dir_all(&thumbnail_directory).map_err(error_string)?;
    let mut command = Command::new(&executable);
    command
        .args(["snapshot", "--json", "--thumbnail-dir"])
        .arg(&thumbnail_directory);
    configure_background_process(&mut command);
    let output = command.output().map_err(error_string)?;
    if !output.status.success() {
        return Err(format!(
            "{} exited with {}: {}",
            executable.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut candidates: Vec<WindowCandidate> =
        serde_json::from_slice(&output.stdout).map_err(error_string)?;
    let active_targets: HashSet<String> = active_agent_leases(unix_ms())
        .map_err(error_string)?
        .into_iter()
        .map(|lease| lease.target_key)
        .collect();
    candidates.retain(|candidate| {
        candidate.visible || active_targets.contains(&candidate.native_target.key())
    });
    Ok(candidates)
}

#[tauri::command]
pub fn list_agents() -> Result<Vec<AgentLease>, String> {
    active_agent_leases(unix_ms()).map_err(error_string)
}

#[tauri::command]
pub fn default_output_directory() -> Result<String, String> {
    let path = if cfg!(target_os = "windows") {
        env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .map(|value| value.join("Videos/OmniInlet"))
    } else if cfg!(target_os = "macos") {
        env::var_os("HOME")
            .map(PathBuf::from)
            .map(|value| value.join("Movies/OmniInlet"))
    } else {
        linux_videos_directory().or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|value| value.join("Videos/OmniInlet"))
        })
    }
    .ok_or_else(|| "无法确定当前用户的视频目录".to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn start_capture(
    app: AppHandle,
    state: State<'_, Supervisor>,
    request: StartCaptureRequest,
) -> Result<String, String> {
    ensure_capture_permission()?;
    if request.output_root.trim().is_empty() {
        return Err("输出目录不能为空".into());
    }
    let job_id = format!("{:X}-{}", unix_ms(), std::process::id());
    let output_directory = PathBuf::from(&request.output_root)
        .join(Local::now().format("%Y-%m-%d").to_string())
        .join(&job_id);
    std::fs::create_dir_all(&output_directory).map_err(error_string)?;

    let executable = resolve_program(&app, "capture-agent")?;
    let mut command = Command::new(&executable);
    command
        .args([
            "run",
            "--target-kind",
            &request.target.kind,
            "--window-id",
            &request.target.value,
        ])
        .arg("--output")
        .arg(&output_directory)
        .args([
            "--job-id",
            &job_id,
            "--fps",
            &request.fps.to_string(),
            "--segment-seconds",
            &request.segment_seconds.to_string(),
            "--video-bitrate-kbps",
            &request.bitrate_kbps.to_string(),
            "--segments",
            "0",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_background_process(&mut command);
    let mut child = command.spawn().map_err(error_string)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法读取采集器输出".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法读取采集器错误输出".to_string())?;
    let handle = Arc::new(Mutex::new(child));
    state
        .children
        .lock()
        .map_err(error_string)?
        .insert(job_id.clone(), Arc::clone(&handle));

    let event_job_id = job_id.clone();
    let event_title = request.title;
    let event_app = app.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let payload = serde_json::from_str::<serde_json::Value>(&line)
                .unwrap_or_else(|_| serde_json::json!({ "event": "agent_log", "message": line }));
            let _ = event_app.emit("capture-event", payload);
        }
        let error_text = BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
            .collect::<Vec<_>>()
            .join("\n");
        let status = handle.lock().ok().and_then(|mut child| child.wait().ok());
        let _ = event_app.emit(
            "capture-exited",
            serde_json::json!({
                "jobId": event_job_id,
                "windowTitle": event_title,
                "exitCode": status.and_then(|value| value.code()),
                "error": error_text,
            }),
        );
        if let Ok(mut children) = event_app.state::<Supervisor>().children.lock() {
            children.remove(&event_job_id);
        }
    });

    Ok(job_id)
}

#[tauri::command]
pub fn stop_capture(state: State<'_, Supervisor>, job_id: String) -> Result<(), String> {
    let handle = state
        .children
        .lock()
        .map_err(error_string)?
        .get(&job_id)
        .cloned()
        .ok_or_else(|| format!("采集任务不存在或已经结束：{job_id}"))?;
    handle
        .lock()
        .map_err(error_string)?
        .kill()
        .map_err(error_string)
}

fn resolve_program(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    let executable_name = if cfg!(target_os = "windows") {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let current = env::current_exe().map_err(error_string)?;
    let packaged = current
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("bin")
        .join(&executable_name);
    if packaged.is_file() {
        return Ok(packaged);
    }
    let resource_sidecar = app
        .path()
        .resource_dir()
        .map_err(error_string)?
        .join("bin")
        .join(&executable_name);
    if resource_sidecar.is_file() {
        return Ok(resource_sidecar);
    }
    let development = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../capture-agent/target/debug")
        .join(&executable_name);
    development
        .is_file()
        .then_some(development)
        .ok_or_else(|| format!("找不到采集子程序：{executable_name}，请先运行 cargo xtask build"))
}

fn runtime_directory() -> anyhow::Result<PathBuf> {
    Ok(agents_directory()?
        .parent()
        .expect("agents directory always has a parent")
        .to_path_buf())
}

fn linux_videos_directory() -> Option<PathBuf> {
    let config = env::var_os("HOME")
        .map(PathBuf::from)?
        .join(".config/user-dirs.dirs");
    let content = std::fs::read_to_string(config).ok()?;
    let value = content
        .lines()
        .find_map(|line| line.strip_prefix("XDG_VIDEOS_DIR="))?
        .trim_matches('"')
        .replace("$HOME", &env::var("HOME").ok()?);
    Some(PathBuf::from(value).join("OmniInlet"))
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn error_string(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn configure_background_process(command: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = command;
}

fn ensure_capture_permission() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    if !crate::permissions::capture_permission_status().granted {
        return Err("缺少 macOS 屏幕录制权限，请授权并重新启动 OmniInlet".into());
    }
    Ok(())
}
