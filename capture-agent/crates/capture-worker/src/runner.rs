use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    ffmpeg::SegmentEncoder,
    registry::LeaseGuard,
    source::{CaptureSource, CapturedFrame, SourceState},
};
use anyhow::{Context, Result, ensure};
use capture_protocol::{
    AgentState, CaptureConfiguration, CaptureEvent, CaptureJobEvents, CaptureJobMeta, NativeTarget,
    PROTOCOL_VERSION, VideoEncodingConfiguration,
};

const META_FILE: &str = "meta.json";
const EVENTS_FILE: &str = "events.json";
const VIDEOS_DIRECTORY: &str = "videos";

#[derive(Debug, Clone)]
pub struct CaptureOptions {
    pub job_id: String,
    pub output_directory: PathBuf,
    pub segment_duration: Duration,
    pub fps: u32,
    pub video_bitrate_kbps: u32,
    pub max_segments: Option<u64>,
    pub stop_requested: Arc<AtomicBool>,
}

pub struct CaptureSummary {
    pub job_directory: PathBuf,
    pub segments: u64,
    pub frames: u64,
}

#[derive(Debug)]
pub struct SourceLostError;

impl std::fmt::Display for SourceLostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the native capture target no longer exists")
    }
}

impl std::error::Error for SourceLostError {}

pub fn run_capture(
    source: &mut dyn CaptureSource,
    options: &CaptureOptions,
) -> Result<CaptureSummary> {
    validate_options(options)?;
    fs::create_dir_all(&options.output_directory)?;
    let videos_directory = options.output_directory.join(VIDEOS_DIRECTORY);
    fs::create_dir_all(&videos_directory)?;

    let source_info = source.info();
    let target = NativeTarget {
        kind: source_info.kind.target_kind().to_string(),
        value: source_info.id.clone(),
    };
    let agent_id = format!("{}-{}", options.job_id, std::process::id());
    let mut lease = LeaseGuard::create(
        agent_id.clone(),
        options.job_id.clone(),
        target,
        &options.output_directory,
    )?;
    lease.update(AgentState::Capturing, 0, 0)?;
    emit_runtime_event(serde_json::json!({
        "event": "agent_started",
        "agentId": agent_id,
        "jobId": options.job_id,
        "pid": std::process::id()
    }));
    let encoded_width = source_info.width.next_multiple_of(2);
    let encoded_height = source_info.height.next_multiple_of(2);
    let key_frame_interval_frames = options
        .fps
        .saturating_mul(options.segment_duration.as_secs().max(1) as u32);
    let meta = CaptureJobMeta {
        schema_version: PROTOCOL_VERSION,
        job_id: options.job_id.clone(),
        created_at_unix_ms: unix_ms(),
        source: source_info.clone(),
        capture: CaptureConfiguration {
            width: source_info.width,
            height: source_info.height,
            fps: options.fps,
        },
        video: VideoEncodingConfiguration {
            container: "matroska".to_string(),
            file_extension: "mkv".to_string(),
            codec: "h264".to_string(),
            framework: "ffmpeg-dynamic".to_string(),
            encoder: "libopenh264".to_string(),
            pixel_format: "yuv420p".to_string(),
            rate_control: "cbr".to_string(),
            bitrate_kbps: options.video_bitrate_kbps,
            speed_preset: "default".to_string(),
            tune: "none".to_string(),
            audio: false,
            encoded_width,
            encoded_height,
            segment_duration_ms: options.segment_duration.as_millis() as u64,
            key_frame_interval_frames,
        },
    };

    let meta_path = options.output_directory.join(META_FILE);
    let is_resuming = meta_path.exists();
    if is_resuming {
        let existing: CaptureJobMeta = read_json(&meta_path)?;
        ensure!(
            existing.job_id == meta.job_id
                && existing.source == meta.source
                && existing.capture == meta.capture
                && existing.video == meta.video,
            "existing meta.json does not match this capture request"
        );
    } else {
        write_json_atomic(&meta_path, &meta)?;
    }

    let events_path = options.output_directory.join(EVENTS_FILE);
    let mut events = if events_path.exists() {
        let existing: CaptureJobEvents = read_json(&events_path)?;
        ensure!(
            existing.job_id == options.job_id,
            "existing events.json belongs to a different job"
        );
        existing
    } else {
        CaptureJobEvents::new(&options.job_id)
    };
    events.push(if is_resuming {
        CaptureEvent::JobResumed {
            occurred_at_unix_ms: unix_ms(),
            pid: std::process::id(),
        }
    } else {
        CaptureEvent::JobStarted {
            occurred_at_unix_ms: unix_ms(),
            pid: std::process::id(),
        }
    });
    write_json_atomic(&events_path, &events)?;

    let mut segment_sequence = next_segment_sequence(&videos_directory)?;
    let mut segments_written = 0_u64;
    let mut frames_written = 0_u64;
    let frame_interval = Duration::from_secs_f64(1.0 / f64::from(options.fps));
    let capture_started = Instant::now();
    let mut last_heartbeat = Instant::now();

    loop {
        if options.stop_requested.load(Ordering::Relaxed)
            || options
                .max_segments
                .is_some_and(|maximum| segments_written >= maximum)
        {
            break;
        }

        let segment_started = Instant::now();
        let segment_started_at_unix_ms = unix_ms();
        let temporary_path = videos_directory.join(format!(".{segment_sequence:08}.mkv.tmp"));
        let final_path = videos_directory.join(format!("{segment_sequence:08}.mkv"));
        ensure!(
            !final_path.exists(),
            "video segment already exists: {}",
            final_path.display()
        );

        let mut encoder = SegmentEncoder::start(
            &temporary_path,
            source_info.width,
            source_info.height,
            options.fps,
            key_frame_interval_frames,
            options.video_bitrate_kbps,
        )?;
        let mut segment_frames = 0_u64;
        let mut next_frame_at = Instant::now();
        let mut capture_error = None;

        while segment_started.elapsed() < options.segment_duration
            && !options.stop_requested.load(Ordering::Relaxed)
        {
            match source.capture() {
                Ok(frame) => {
                    validate_frame(&frame, source_info.width, source_info.height)?;
                    encoder.write_frame(&frame)?;
                    segment_frames += 1;
                    frames_written += 1;
                }
                Err(error) => {
                    capture_error = Some(error);
                    break;
                }
            }

            if last_heartbeat.elapsed() >= Duration::from_secs(2) {
                lease.update(
                    AgentState::Capturing,
                    segments_written,
                    capture_started.elapsed().as_millis() as u64,
                )?;
                emit_runtime_event(serde_json::json!({
                    "event": "heartbeat",
                    "agentId": agent_id,
                    "segments": segments_written,
                    "recordedDurationMs": capture_started.elapsed().as_millis() as u64
                }));
                last_heartbeat = Instant::now();
            }

            next_frame_at += frame_interval;
            if let Some(delay) = next_frame_at.checked_duration_since(Instant::now()) {
                thread::sleep(delay);
            } else {
                next_frame_at = Instant::now();
            }
        }

        if segment_frames == 0 {
            encoder.abort()?;
            remove_file_if_exists(&temporary_path)?;
            if let Some(error) = capture_error {
                match source.state()? {
                    SourceState::Hidden => {
                        wait_for_source(
                            source,
                            options,
                            &mut lease,
                            &agent_id,
                            &mut events,
                            &events_path,
                            segments_written,
                            capture_started,
                        )?;
                        continue;
                    }
                    SourceState::Destroyed => {
                        record_source_lost(&mut events, &events_path, &agent_id)?;
                        return Err(SourceLostError.into());
                    }
                    SourceState::Available => {
                        record_gap(&mut events, &events_path, &error.to_string())?;
                        return Err(error).context("capture source failed while target is alive");
                    }
                }
            }
            break;
        }

        encoder.finish()?;
        sync_file(&temporary_path)?;
        fs::rename(&temporary_path, &final_path)?;

        let ended_at_unix_ms = unix_ms();
        events.push(CaptureEvent::VideoSegmentCompleted {
            sequence: segment_sequence,
            path: format!("videos/{segment_sequence:08}.mkv"),
            started_at_unix_ms: segment_started_at_unix_ms,
            ended_at_unix_ms,
            duration_ms: segment_started.elapsed().as_millis() as u64,
            frame_count: segment_frames,
            bytes: fs::metadata(&final_path)?.len(),
        });
        write_json_atomic(&events_path, &events)?;
        segments_written += 1;
        lease.update(
            AgentState::Capturing,
            segments_written,
            capture_started.elapsed().as_millis() as u64,
        )?;
        emit_runtime_event(serde_json::json!({
            "event": "video_segment_completed",
            "agentId": agent_id,
            "sequence": segment_sequence,
            "path": final_path,
            "segments": segments_written
        }));
        segment_sequence += 1;

        if let Some(error) = capture_error {
            match source.state()? {
                SourceState::Hidden => wait_for_source(
                    source,
                    options,
                    &mut lease,
                    &agent_id,
                    &mut events,
                    &events_path,
                    segments_written,
                    capture_started,
                )?,
                SourceState::Destroyed => {
                    record_source_lost(&mut events, &events_path, &agent_id)?;
                    return Err(SourceLostError.into());
                }
                SourceState::Available => {
                    record_gap(&mut events, &events_path, &error.to_string())?;
                    return Err(error).context("capture source failed while target is alive");
                }
            }
        }
    }

    events.push(CaptureEvent::JobStopped {
        occurred_at_unix_ms: unix_ms(),
        segments: segments_written,
        frames: frames_written,
    });
    write_json_atomic(&events_path, &events)?;
    emit_runtime_event(serde_json::json!({
        "event": "capture_stopped",
        "agentId": agent_id,
        "segments": segments_written,
        "frames": frames_written
    }));

    Ok(CaptureSummary {
        job_directory: options.output_directory.clone(),
        segments: segments_written,
        frames: frames_written,
    })
}

#[allow(clippy::too_many_arguments)]
fn wait_for_source(
    source: &dyn CaptureSource,
    options: &CaptureOptions,
    lease: &mut LeaseGuard,
    agent_id: &str,
    events: &mut CaptureJobEvents,
    events_path: &Path,
    segments: u64,
    capture_started: Instant,
) -> Result<()> {
    events.push(CaptureEvent::CaptureSuspended {
        reason: "window_hidden".to_string(),
        occurred_at_unix_ms: unix_ms(),
    });
    write_json_atomic(events_path, events)?;
    lease.update(
        AgentState::Suspended,
        segments,
        capture_started.elapsed().as_millis() as u64,
    )?;
    emit_runtime_event(serde_json::json!({
        "event": "capture_suspended",
        "agentId": agent_id,
        "reason": "window_hidden"
    }));

    loop {
        if options.stop_requested.load(Ordering::Relaxed) {
            return Ok(());
        }
        thread::sleep(Duration::from_secs(2));
        match source.state()? {
            SourceState::Available => {
                events.push(CaptureEvent::CaptureResumed {
                    occurred_at_unix_ms: unix_ms(),
                });
                write_json_atomic(events_path, events)?;
                lease.update(
                    AgentState::Capturing,
                    segments,
                    capture_started.elapsed().as_millis() as u64,
                )?;
                emit_runtime_event(serde_json::json!({
                    "event": "capture_resumed",
                    "agentId": agent_id
                }));
                return Ok(());
            }
            SourceState::Hidden => lease.update(
                AgentState::Suspended,
                segments,
                capture_started.elapsed().as_millis() as u64,
            )?,
            SourceState::Destroyed => {
                record_source_lost(events, events_path, agent_id)?;
                return Err(SourceLostError.into());
            }
        }
    }
}

fn record_source_lost(
    events: &mut CaptureJobEvents,
    events_path: &Path,
    agent_id: &str,
) -> Result<()> {
    events.push(CaptureEvent::SourceLost {
        occurred_at_unix_ms: unix_ms(),
    });
    write_json_atomic(events_path, events)?;
    emit_runtime_event(serde_json::json!({
        "event": "source_lost",
        "agentId": agent_id
    }));
    Ok(())
}

fn emit_runtime_event(value: serde_json::Value) {
    println!("{value}");
    let _ = std::io::stdout().flush();
}

fn validate_options(options: &CaptureOptions) -> Result<()> {
    ensure!(
        !options.job_id.trim().is_empty(),
        "job id must not be empty"
    );
    ensure!(options.fps > 0, "fps must be greater than zero");
    ensure!(
        options.video_bitrate_kbps > 0,
        "video bitrate must be greater than zero"
    );
    ensure!(
        !options.segment_duration.is_zero(),
        "segment duration must be greater than zero"
    );
    Ok(())
}

fn validate_frame(frame: &CapturedFrame, width: u32, height: u32) -> Result<()> {
    ensure!(
        frame.width == width && frame.height == height,
        "capture dimensions changed from {width}x{height} to {}x{}",
        frame.width,
        frame.height
    );
    let expected = width as usize * height as usize * 4;
    ensure!(
        frame.rgba.len() == expected,
        "RGBA frame has {} bytes, expected {expected}",
        frame.rgba.len()
    );
    Ok(())
}

fn record_gap(events: &mut CaptureJobEvents, events_path: &Path, reason: &str) -> Result<()> {
    events.push(CaptureEvent::CaptureGap {
        reason: reason.to_string(),
        occurred_at_unix_ms: unix_ms(),
    });
    write_json_atomic(events_path, events)
}

fn next_segment_sequence(videos_directory: &Path) -> Result<u64> {
    let mut maximum = 0_u64;
    for entry in fs::read_dir(videos_directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("mkv") {
            continue;
        }
        if let Some(sequence) = path
            .file_stem()
            .and_then(|value| value.to_str())
            .and_then(|value| value.parse::<u64>().ok())
        {
            maximum = maximum.max(sequence);
        }
    }
    Ok(maximum + 1)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    serde_json::from_reader(file).with_context(|| format!("cannot parse {}", path.display()))
}

fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let temporary_path = path.with_extension("json.tmp");
    {
        let file = File::create(&temporary_path)?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, value)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
    }
    replace_file(&temporary_path, path)?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    fs::rename(source, destination)?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::Storage::FileSystem::{
            MOVE_FILE_FLAGS, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        },
        core::PCWSTR,
    };

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let flags = MOVE_FILE_FLAGS(MOVEFILE_REPLACE_EXISTING.0 | MOVEFILE_WRITE_THROUGH.0);
    unsafe { MoveFileExW(PCWSTR(source.as_ptr()), PCWSTR(destination.as_ptr()), flags) }
        .context("cannot atomically replace JSON document")
}

fn sync_file(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::test_pattern::TestPatternSource;

    #[test]
    #[ignore = "requires capture-runtime; the packaged executable integration test covers this path"]
    fn commits_mkv_segments_and_json_documents() {
        let directory = tempdir().unwrap();
        let job_directory = directory.path().join("job-test");
        let mut source = TestPatternSource::new(81, 45);
        let summary = run_capture(
            &mut source,
            &CaptureOptions {
                job_id: "job-test".to_string(),
                output_directory: job_directory.clone(),
                segment_duration: Duration::from_millis(120),
                fps: 20,
                video_bitrate_kbps: 2048,
                max_segments: Some(2),
                stop_requested: Arc::new(AtomicBool::new(false)),
            },
        )
        .unwrap();

        assert_eq!(summary.job_directory, job_directory);
        assert_eq!(summary.segments, 2);
        assert!(summary.frames >= 4);
        assert!(job_directory.join(META_FILE).is_file());
        assert!(job_directory.join(EVENTS_FILE).is_file());
        for sequence in 1..=2 {
            let path = job_directory.join(format!("videos/{sequence:08}.mkv"));
            let video = fs::read(&path).unwrap();
            assert!(video.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]));
            assert!(video.len() > 256);
        }

        let meta: CaptureJobMeta = read_json(&job_directory.join(META_FILE)).unwrap();
        assert_eq!(meta.video.codec, "h264");
        assert_eq!(meta.video.container, "matroska");
        assert_eq!(meta.video.encoded_width, 82);
        assert_eq!(meta.video.encoded_height, 46);

        let events: CaptureJobEvents = read_json(&job_directory.join(EVENTS_FILE)).unwrap();
        assert_eq!(events.job_id, "job-test");
        assert_eq!(events.events.len(), 4);
        assert!(matches!(events.events[0], CaptureEvent::JobStarted { .. }));
        assert!(matches!(
            events.events[1],
            CaptureEvent::VideoSegmentCompleted { sequence: 1, .. }
        ));
        assert!(matches!(events.events[3], CaptureEvent::JobStopped { .. }));

        let resumed = run_capture(
            &mut source,
            &CaptureOptions {
                job_id: "job-test".to_string(),
                output_directory: job_directory.clone(),
                segment_duration: Duration::from_millis(120),
                fps: 20,
                video_bitrate_kbps: 2048,
                max_segments: Some(1),
                stop_requested: Arc::new(AtomicBool::new(false)),
            },
        )
        .unwrap();
        assert_eq!(resumed.segments, 1);
        assert!(job_directory.join("videos/00000003.mkv").is_file());
        let events: CaptureJobEvents = read_json(&job_directory.join(EVENTS_FILE)).unwrap();
        assert_eq!(events.events.len(), 7);
        assert!(matches!(events.events[4], CaptureEvent::JobResumed { .. }));
        assert!(matches!(
            events.events[5],
            CaptureEvent::VideoSegmentCompleted { sequence: 3, .. }
        ));
    }
}
