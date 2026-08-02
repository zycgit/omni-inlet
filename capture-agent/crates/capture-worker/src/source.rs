use anyhow::Result;
use capture_protocol::CaptureSourceInfo;

pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceState {
    Available,
    Hidden,
    Destroyed,
}

pub trait CaptureSource {
    fn info(&self) -> CaptureSourceInfo;
    fn capture(&mut self) -> Result<CapturedFrame>;
    fn state(&self) -> Result<SourceState> {
        Ok(SourceState::Available)
    }
}
