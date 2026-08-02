use std::{fs, path::Path};

use anyhow::{Result, bail};
use capture_protocol::{ApplicationInfo, NativeTarget, WindowCandidate};

use crate::{
    source::CaptureSource,
    x11::{X11WindowSource, list_window_details, parse_window_id},
};

pub fn enumerate_windows(thumbnail_directory: Option<&Path>) -> Result<Vec<WindowCandidate>> {
    if let Some(directory) = thumbnail_directory {
        fs::create_dir_all(directory)?;
    }
    let mut candidates = Vec::new();
    for detail in list_window_details()? {
        let source = detail.source;
        let target = NativeTarget {
            kind: "x11-window".to_string(),
            value: source.id.clone(),
        };
        let thumbnail_path = thumbnail_directory.and_then(|directory| {
            let path = directory.join(format!("x11-{}.png", source.id.trim_start_matches("0x")));
            let window = parse_window_id(&source.id).ok()?;
            let mut capture = X11WindowSource::connect(window).ok()?;
            let frame = capture.capture().ok()?;
            super::save_thumbnail(&frame.rgba, frame.width, frame.height, &path).ok()?;
            Some(path.to_string_lossy().into_owned())
        });
        candidates.push(WindowCandidate {
            candidate_id: target.key(),
            application: ApplicationInfo {
                group_id: format!("linux:{}", detail.application_name.to_ascii_lowercase()),
                display_name: detail.application_name,
                process_id: detail.process_id,
                icon_path: None,
            },
            title: source.title,
            visible: source.visible,
            capturable: source.visible,
            unavailable_reason: (!source.visible).then(|| "窗口已隐藏".to_string()),
            thumbnail_path,
            width: source.width,
            height: source.height,
            native_target: target,
        });
    }
    Ok(candidates)
}

pub fn open_window_source(kind: &str, value: &str) -> Result<Box<dyn CaptureSource>> {
    if kind != "x11-window" {
        bail!("unsupported Linux target kind: {kind}");
    }
    Ok(Box::new(X11WindowSource::connect(parse_window_id(value)?)?))
}
