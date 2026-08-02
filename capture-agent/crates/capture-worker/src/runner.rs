use std::{
    ffi::OsString,
    fs::{self, File},
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use capture_protocol::{
    CaptureConfiguration, CaptureEvent, CaptureJobEvents, CaptureJobMeta, PROTOCOL_VERSION,
    VideoEncodingConfiguration,
};

use crate::source::{CaptureSource, CapturedFrame};

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
    pub gstreamer_launch: OsString,
    pub stop_requested: Arc<AtomicBool>,
}

pub struct CaptureSummary {
    pub job_directory: PathBuf,
    pub segments: u64,
    pub frames: u64,
}

pub fn run_capture(
    source: &mut dyn CaptureSource,
    options: &CaptureOptions,
) -> Result<CaptureSummary> {
    validate_options(options)?;
    verify_gstreamer(&options.gstreamer_launch)?;

    fs::create_dir_all(&options.output_directory)?;
    let videos_directory = options.output_directory.join(VIDEOS_DIRECTORY);
    fs::create_dir_all(&videos_directory)?;

    let source_info = source.info();
    let encoded_width = even_dimension(source_info.width)?;
    let encoded_height = even_dimension(source_info.height)?;
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
            framework: "gstreamer".to_string(),
            encoder: "x264enc".to_string(),
            pixel_format: "yuv420p".to_string(),
            rate_control: "cbr".to_string(),
            bitrate_kbps: options.video_bitrate_kbps,
            speed_preset: "veryfast".to_string(),
            tune: "zerolatency".to_string(),
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
            &options.gstreamer_launch,
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
                record_gap(&mut events, &events_path, &error.to_string())?;
                return Err(error).context("capture source failed");
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
        segment_sequence += 1;

        if let Some(error) = capture_error {
            record_gap(&mut events, &events_path, &error.to_string())?;
            return Err(error).context("capture source failed");
        }
    }

    events.push(CaptureEvent::JobStopped {
        occurred_at_unix_ms: unix_ms(),
        segments: segments_written,
        frames: frames_written,
    });
    write_json_atomic(&events_path, &events)?;

    Ok(CaptureSummary {
        job_directory: options.output_directory.clone(),
        segments: segments_written,
        frames: frames_written,
    })
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

fn verify_gstreamer(program: &OsString) -> Result<()> {
    let output = Command::new(program)
        .arg("--version")
        .output()
        .with_context(|| format!("cannot start {}", PathBuf::from(program).display()))?;
    ensure!(
        output.status.success(),
        "GStreamer launcher exited with {}",
        output.status
    );
    let launch_path = PathBuf::from(program);
    let launch_name = launch_path
        .file_name()
        .and_then(|value| value.to_str())
        .context("GStreamer launcher path has no valid file name")?;
    let inspect_name = launch_name.replacen("gst-launch", "gst-inspect", 1);
    ensure!(
        inspect_name != launch_name,
        "cannot derive gst-inspect command from {launch_name}"
    );
    let inspect_program = launch_path.with_file_name(inspect_name);
    for element in [
        "fdsrc",
        "rawvideoparse",
        "videoconvert",
        "videobox",
        "x264enc",
        "matroskamux",
        "filesink",
    ] {
        let status = Command::new(&inspect_program)
            .arg(element)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .with_context(|| format!("cannot start {}", inspect_program.display()))?;
        ensure!(
            status.success(),
            "required GStreamer element is missing: {element}"
        );
    }
    Ok(())
}

fn even_dimension(value: u32) -> Result<u32> {
    ensure!(value > 0, "capture dimension must be greater than zero");
    value
        .checked_add(value % 2)
        .context("capture dimension is too large")
}

struct SegmentEncoder {
    child: Child,
    stdin: Option<BufWriter<ChildStdin>>,
}

impl SegmentEncoder {
    fn start(
        program: &OsString,
        output: &Path,
        width: u32,
        height: u32,
        fps: u32,
        key_frame_interval_frames: u32,
        video_bitrate_kbps: u32,
    ) -> Result<Self> {
        let block_size = width as u64 * height as u64 * 4;
        let encoded_width = even_dimension(width)?;
        let encoded_height = even_dimension(height)?;
        let right_padding = encoded_width - width;
        let bottom_padding = encoded_height - height;
        let mut child = Command::new(program)
            .args(["-q", "fdsrc", "fd=0"])
            .arg(format!("blocksize={block_size}"))
            .args(["!", "rawvideoparse", "format=rgba"])
            .arg(format!("width={width}"))
            .arg(format!("height={height}"))
            .arg(format!("framerate={fps}/1"))
            .args(["!", "videoconvert", "!", "videobox"])
            .arg(format!("right=-{right_padding}"))
            .arg(format!("bottom=-{bottom_padding}"))
            .arg("!")
            .arg(format!(
                "video/x-raw,format=I420,width={encoded_width},height={encoded_height}"
            ))
            .args(["!", "x264enc", "tune=zerolatency", "speed-preset=veryfast"])
            .arg(format!("key-int-max={key_frame_interval_frames}"))
            .arg(format!("bitrate={video_bitrate_kbps}"))
            .args(["!", "matroskamux", "!", "filesink"])
            .arg(format!("location={}", output.display()))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("cannot start GStreamer video encoder")?;
        let stdin = child
            .stdin
            .take()
            .context("GStreamer stdin is unavailable")?;
        Ok(Self {
            child,
            stdin: Some(BufWriter::new(stdin)),
        })
    }

    fn write_frame(&mut self, frame: &CapturedFrame) -> Result<()> {
        self.stdin
            .as_mut()
            .context("video encoder is already closed")?
            .write_all(&frame.rgba)
            .context("cannot send frame to GStreamer")
    }

    fn finish(mut self) -> Result<()> {
        if let Some(mut stdin) = self.stdin.take() {
            stdin.flush()?;
            drop(stdin);
        }
        let status = self.child.wait()?;
        let mut stderr = String::new();
        if let Some(mut pipe) = self.child.stderr.take() {
            pipe.read_to_string(&mut stderr)?;
        }
        if !status.success() {
            bail!(
                "GStreamer encoder failed with {}: {}",
                status,
                stderr.trim()
            );
        }
        Ok(())
    }

    fn abort(mut self) -> Result<()> {
        self.stdin.take();
        if self.child.try_wait()?.is_none() {
            self.child.kill()?;
        }
        self.child.wait()?;
        Ok(())
    }
}

impl Drop for SegmentEncoder {
    fn drop(&mut self) {
        self.stdin.take();
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
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
    fs::rename(temporary_path, path)?;
    Ok(())
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
    fn commits_mkv_segments_and_json_documents() {
        if verify_gstreamer(&"gst-launch-1.0".into()).is_err() {
            return;
        }

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
                gstreamer_launch: "gst-launch-1.0".into(),
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
            let video = fs::read(job_directory.join(format!("videos/{sequence:08}.mkv"))).unwrap();
            assert!(video.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]));
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
                gstreamer_launch: "gst-launch-1.0".into(),
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
