use std::{path::Path, sync::OnceLock};

use anyhow::{Result, bail};

use crate::source::CapturedFrame;

pub trait VideoEncoder {
    fn write_frame(&mut self, frame: &CapturedFrame) -> Result<()>;
    fn finish(self: Box<Self>) -> Result<()>;
    fn abort(self: Box<Self>) -> Result<()>;
}

pub type EncoderFactory = fn(&Path, u32, u32, u32, u32, u32) -> Result<Box<dyn VideoEncoder>>;

static ENCODER_FACTORY: OnceLock<EncoderFactory> = OnceLock::new();

pub fn install_encoder_factory(factory: EncoderFactory) -> Result<()> {
    if let Some(installed) = ENCODER_FACTORY.get() {
        if std::ptr::fn_addr_eq(*installed, factory) {
            return Ok(());
        }
        bail!("a different video encoder factory is already installed");
    }
    ENCODER_FACTORY
        .set(factory)
        .map_err(|_| anyhow::anyhow!("cannot install video encoder factory"))
}

pub fn verify_video_runtime() -> Result<()> {
    if ENCODER_FACTORY.get().is_none() {
        bail!("capture-runtime did not install its FFmpeg encoder");
    }
    Ok(())
}

pub struct SegmentEncoder {
    inner: Box<dyn VideoEncoder>,
}

impl SegmentEncoder {
    pub fn start(
        output: &Path,
        width: u32,
        height: u32,
        fps: u32,
        key_frame_interval: u32,
        bitrate_kbps: u32,
    ) -> Result<Self> {
        let factory = ENCODER_FACTORY
            .get()
            .ok_or_else(|| anyhow::anyhow!("capture-runtime FFmpeg encoder is not installed"))?;
        Ok(Self {
            inner: factory(output, width, height, fps, key_frame_interval, bitrate_kbps)?,
        })
    }

    pub fn write_frame(&mut self, frame: &CapturedFrame) -> Result<()> {
        self.inner.write_frame(frame)
    }

    pub fn finish(self) -> Result<()> {
        self.inner.finish()
    }

    pub fn abort(self) -> Result<()> {
        self.inner.abort()
    }
}
