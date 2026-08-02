use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use capture_protocol::{AgentLease, AgentState, NativeTarget, PROTOCOL_VERSION};

const STALE_AFTER_MS: u64 = 6_000;

pub struct LeaseGuard {
    path: PathBuf,
    lease: AgentLease,
}

impl LeaseGuard {
    pub fn create(
        agent_id: String,
        job_id: String,
        target: NativeTarget,
        output_directory: &Path,
    ) -> Result<Self> {
        let directory = agents_directory()?;
        fs::create_dir_all(&directory)?;
        let now = unix_ms();
        let lease = AgentLease {
            schema_version: PROTOCOL_VERSION,
            agent_id: agent_id.clone(),
            job_id,
            pid: std::process::id(),
            target_key: target.key(),
            target,
            output_directory: output_directory.to_string_lossy().into_owned(),
            state: AgentState::Starting,
            started_at_unix_ms: now,
            heartbeat_at_unix_ms: now,
            segments: 0,
            recorded_duration_ms: 0,
        };
        let mut guard = Self {
            path: directory.join(format!("{agent_id}.json")),
            lease,
        };
        guard.flush()?;
        Ok(guard)
    }

    pub fn update(&mut self, state: AgentState, segments: u64, duration_ms: u64) -> Result<()> {
        self.lease.state = state;
        self.lease.segments = segments;
        self.lease.recorded_duration_ms = duration_ms;
        self.lease.heartbeat_at_unix_ms = unix_ms();
        self.flush()
    }

    fn flush(&mut self) -> Result<()> {
        write_json_atomic(&self.path, &self.lease)
    }
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn agents_directory() -> Result<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Library/Application Support"))
    } else {
        env::var_os("XDG_RUNTIME_DIR")
            .or_else(|| env::var_os("XDG_CACHE_HOME"))
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
    }
    .context("cannot resolve the current user's runtime directory")?;
    Ok(base
        .join(if cfg!(target_os = "linux") {
            "omni-inlet"
        } else {
            "OmniInlet"
        })
        .join("runtime/agents"))
}

pub fn active_leases(now_ms: u64) -> Result<Vec<AgentLease>> {
    let directory = agents_directory()?;
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut leases = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        let lease: AgentLease = match serde_json::from_slice(&bytes) {
            Ok(lease) => lease,
            Err(_) => continue,
        };
        if now_ms.saturating_sub(lease.heartbeat_at_unix_ms) <= STALE_AFTER_MS {
            leases.push(lease);
        }
    }
    Ok(leases)
}

pub fn counts_by_target(leases: &[AgentLease]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for lease in leases {
        *counts.entry(lease.target_key.clone()).or_insert(0) += 1;
    }
    counts
}

fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(agent: &str, target: &str) -> AgentLease {
        let native = NativeTarget {
            kind: "windows-hwnd".into(),
            value: target.into(),
        };
        AgentLease {
            schema_version: 1,
            agent_id: agent.into(),
            job_id: agent.into(),
            pid: 1,
            target_key: native.key(),
            target: native,
            output_directory: "out".into(),
            state: AgentState::Capturing,
            started_at_unix_ms: 1,
            heartbeat_at_unix_ms: 1,
            segments: 0,
            recorded_duration_ms: 0,
        }
    }

    #[test]
    fn counts_multiple_agents_for_the_same_window() {
        let counts = counts_by_target(&[
            lease("a", "0x1"),
            lease("b", "0x1"),
            lease("c", "0x2"),
            lease("d", "0x1"),
        ]);
        assert_eq!(counts["windows-hwnd:0x1"], 3);
        assert_eq!(counts["windows-hwnd:0x2"], 1);
    }
}
