use anyhow::Result;
use capture_protocol::CaptureSourceInfo;

pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub trait CaptureSource {
    fn info(&self) -> CaptureSourceInfo;
    fn capture(&mut self) -> Result<CapturedFrame>;
}
