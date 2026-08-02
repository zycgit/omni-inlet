use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSourceKind {
    TestPattern,
    X11Window,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSourceInfo {
    pub kind: CaptureSourceKind,
    pub id: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CaptureConfiguration {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VideoEncodingConfiguration {
    pub container: String,
    pub file_extension: String,
    pub codec: String,
    pub framework: String,
    pub encoder: String,
    pub pixel_format: String,
    pub rate_control: String,
    pub bitrate_kbps: u32,
    pub speed_preset: String,
    pub tune: String,
    pub audio: bool,
    pub encoded_width: u32,
    pub encoded_height: u32,
    pub segment_duration_ms: u64,
    pub key_frame_interval_frames: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CaptureJobMeta {
    pub schema_version: u32,
    pub job_id: String,
    pub created_at_unix_ms: u64,
    pub source: CaptureSourceInfo,
    pub capture: CaptureConfiguration,
    pub video: VideoEncodingConfiguration,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum CaptureEvent {
    #[serde(rename = "job.started")]
    JobStarted {
        #[serde(rename = "occurredAtUnixMs")]
        occurred_at_unix_ms: u64,
        pid: u32,
    },
    #[serde(rename = "job.resumed")]
    JobResumed {
        #[serde(rename = "occurredAtUnixMs")]
        occurred_at_unix_ms: u64,
        pid: u32,
    },
    #[serde(rename = "video.segment.completed")]
    VideoSegmentCompleted {
        sequence: u64,
        path: String,
        #[serde(rename = "startedAtUnixMs")]
        started_at_unix_ms: u64,
        #[serde(rename = "endedAtUnixMs")]
        ended_at_unix_ms: u64,
        #[serde(rename = "durationMs")]
        duration_ms: u64,
        #[serde(rename = "frameCount")]
        frame_count: u64,
        bytes: u64,
    },
    #[serde(rename = "capture.gap")]
    CaptureGap {
        reason: String,
        #[serde(rename = "occurredAtUnixMs")]
        occurred_at_unix_ms: u64,
    },
    #[serde(rename = "job.stopped")]
    JobStopped {
        #[serde(rename = "occurredAtUnixMs")]
        occurred_at_unix_ms: u64,
        segments: u64,
        frames: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CaptureJobEvents {
    pub schema_version: u32,
    pub job_id: String,
    pub revision: u64,
    pub events: Vec<CaptureEvent>,
}

impl CaptureJobEvents {
    pub fn new(job_id: impl Into<String>) -> Self {
        Self {
            schema_version: PROTOCOL_VERSION,
            job_id: job_id.into(),
            revision: 0,
            events: Vec::new(),
        }
    }

    pub fn push(&mut self, event: CaptureEvent) {
        self.events.push(event);
        self.revision += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_a_standard_json_document() {
        let mut document = CaptureJobEvents::new("job-1");
        document.push(CaptureEvent::CaptureGap {
            reason: "source_closed".to_string(),
            occurred_at_unix_ms: 42,
        });

        let value = serde_json::to_value(document).expect("events should serialize");
        assert_eq!(value["jobId"], "job-1");
        assert_eq!(value["revision"], 1);
        assert_eq!(value["events"][0]["type"], "capture.gap");
        assert_eq!(value["events"][0]["reason"], "source_closed");
    }
}
