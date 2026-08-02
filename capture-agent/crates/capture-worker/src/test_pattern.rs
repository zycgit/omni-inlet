use anyhow::Result;
use capture_protocol::{CaptureSourceInfo, CaptureSourceKind};

use crate::source::{CaptureSource, CapturedFrame};

pub struct TestPatternSource {
    width: u32,
    height: u32,
    frame_sequence: u64,
}

impl TestPatternSource {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            frame_sequence: 0,
        }
    }

    fn render(&self) -> Vec<u8> {
        let mut rgba = vec![0_u8; (self.width * self.height * 4) as usize];
        let band_width = (self.width / 8).max(1);
        let shift = (self.frame_sequence as u32 * 7) % self.width.max(1);

        for y in 0..self.height {
            for x in 0..self.width {
                let shifted_x = (x + shift) % self.width.max(1);
                let band = (shifted_x / band_width).min(7);
                let (red, green, blue) = match band {
                    0 => (235, 64, 52),
                    1 => (242, 153, 45),
                    2 => (248, 215, 66),
                    3 => (65, 176, 110),
                    4 => (52, 152, 219),
                    5 => (72, 85, 184),
                    6 => (142, 68, 173),
                    _ => (44, 62, 80),
                };
                let offset = ((y * self.width + x) * 4) as usize;
                rgba[offset] = red;
                rgba[offset + 1] = green;
                rgba[offset + 2] = blue;
                rgba[offset + 3] = 255;
            }
        }

        // Encode the low 32 bits of the frame number as a visible 8x4 grid.
        let cell = (self.width.min(self.height) / 24).max(2);
        for bit in 0..32 {
            let enabled = (self.frame_sequence >> bit) & 1 == 1;
            let grid_x = bit % 8;
            let grid_y = bit / 8;
            let start_x = cell + grid_x as u32 * (cell + 2);
            let start_y = cell + grid_y as u32 * (cell + 2);
            for y in start_y..(start_y + cell).min(self.height) {
                for x in start_x..(start_x + cell).min(self.width) {
                    let offset = ((y * self.width + x) * 4) as usize;
                    let value = if enabled { 255 } else { 0 };
                    rgba[offset] = value;
                    rgba[offset + 1] = value;
                    rgba[offset + 2] = value;
                    rgba[offset + 3] = 255;
                }
            }
        }

        rgba
    }
}

impl CaptureSource for TestPatternSource {
    fn info(&self) -> CaptureSourceInfo {
        CaptureSourceInfo {
            kind: CaptureSourceKind::TestPattern,
            id: "test-pattern".to_string(),
            title: "Deterministic test pattern".to_string(),
            width: self.width,
            height: self.height,
            visible: true,
        }
    }

    fn capture(&mut self) -> Result<CapturedFrame> {
        let rgba = self.render();
        self.frame_sequence += 1;
        Ok(CapturedFrame {
            width: self.width,
            height: self.height,
            rgba,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_is_deterministic_and_changes_between_frames() {
        let mut first = TestPatternSource::new(160, 90);
        let mut second = TestPatternSource::new(160, 90);

        let first_frame = first.capture().expect("first frame");
        let same_frame = second.capture().expect("same frame");
        let next_frame = first.capture().expect("next frame");

        assert_eq!(first_frame.rgba, same_frame.rgba);
        assert_ne!(first_frame.rgba, next_frame.rgba);
        assert_eq!(first_frame.rgba.len(), 160 * 90 * 4);
    }
}
