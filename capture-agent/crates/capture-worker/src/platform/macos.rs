use std::{fs, path::Path};

use anyhow::{Context, Result, bail, ensure};
use capture_protocol::{
    ApplicationInfo, CaptureSourceInfo, CaptureSourceKind, NativeTarget, WindowCandidate,
};
use screencapturekit::{
    screenshot_manager::{CGImageExt, SCScreenshotManager},
    shareable_content::{SCShareableContent, SCWindow},
    stream::{configuration::SCStreamConfiguration, content_filter::SCContentFilter},
};

use crate::source::{CaptureSource, CapturedFrame, SourceState};

pub fn enumerate_windows(thumbnail_directory: Option<&Path>) -> Result<Vec<WindowCandidate>> {
    ensure_screen_capture_permission()?;
    if let Some(directory) = thumbnail_directory {
        fs::create_dir_all(directory)?;
    }
    let content = SCShareableContent::get().map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let mut candidates = Vec::new();
    for window in content.windows() {
        if window.window_layer() != 0 {
            continue;
        }
        let title = window.title().unwrap_or_default();
        if title.trim().is_empty() {
            continue;
        }
        let frame = window.frame();
        let width = frame.size.width.max(0.0).round() as u32;
        let height = frame.size.height.max(0.0).round() as u32;
        if width < 2 || height < 2 {
            continue;
        }
        let app = window.owning_application();
        let display_name = app
            .as_ref()
            .map(|value| value.application_name())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "macOS 应用".into());
        let bundle_id = app
            .as_ref()
            .map(|value| value.bundle_identifier())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| display_name.clone());
        let process_id = app
            .as_ref()
            .and_then(|value| u32::try_from(value.process_id()).ok());
        let visible = window.is_on_screen();
        let value = window.window_id().to_string();
        let target = NativeTarget {
            kind: "macos-cgwindow".into(),
            value: value.clone(),
        };
        let thumbnail_path = if visible {
            thumbnail_directory.and_then(|directory| {
                let path = directory.join(format!("macos-{value}.png"));
                let rgba = screenshot(&window, width.min(960), height.min(600)).ok()?;
                super::save_thumbnail(&rgba, width.min(960), height.min(600), &path).ok()?;
                Some(path.to_string_lossy().into_owned())
            })
        } else {
            None
        };
        candidates.push(WindowCandidate {
            candidate_id: target.key(),
            application: ApplicationInfo {
                group_id: format!("macos:{bundle_id}"),
                display_name,
                process_id,
                icon_path: None,
            },
            title,
            visible,
            capturable: visible,
            unavailable_reason: (!visible).then(|| "窗口已隐藏".into()),
            thumbnail_path,
            width,
            height,
            native_target: target,
        });
    }
    candidates.sort_by(|a, b| {
        a.application
            .display_name
            .cmp(&b.application.display_name)
            .then(a.title.cmp(&b.title))
    });
    Ok(candidates)
}

pub fn open_window_source(kind: &str, value: &str) -> Result<Box<dyn CaptureSource>> {
    ensure_screen_capture_permission()?;
    if kind != "macos-cgwindow" {
        bail!("unsupported macOS target kind: {kind}");
    }
    let window_id = value
        .parse::<u32>()
        .with_context(|| format!("invalid CGWindowID: {value}"))?;
    Ok(Box::new(MacosWindowSource::new(window_id)?))
}

struct MacosWindowSource {
    window_id: u32,
    info: CaptureSourceInfo,
}

impl MacosWindowSource {
    fn new(window_id: u32) -> Result<Self> {
        let window = find_window(window_id)?.context("the target CGWindowID does not exist")?;
        let frame = window.frame();
        let width = frame.size.width.max(0.0).round() as u32;
        let height = frame.size.height.max(0.0).round() as u32;
        ensure!(width > 1 && height > 1, "target window has invalid bounds");
        Ok(Self {
            window_id,
            info: CaptureSourceInfo {
                kind: CaptureSourceKind::MacosWindow,
                id: window_id.to_string(),
                title: window
                    .title()
                    .unwrap_or_else(|| format!("Window {window_id}")),
                width,
                height,
                visible: window.is_on_screen(),
            },
        })
    }
}

impl CaptureSource for MacosWindowSource {
    fn info(&self) -> CaptureSourceInfo {
        self.info.clone()
    }

    fn capture(&mut self) -> Result<CapturedFrame> {
        let window = find_window(self.window_id)?.context("target window was destroyed")?;
        ensure!(window.is_on_screen(), "target window is hidden");
        let frame = window.frame();
        let width = frame.size.width.max(0.0).round() as u32;
        let height = frame.size.height.max(0.0).round() as u32;
        let rgba = screenshot(&window, width, height)?;
        self.info.width = width;
        self.info.height = height;
        Ok(CapturedFrame {
            width,
            height,
            rgba,
        })
    }

    fn state(&self) -> Result<SourceState> {
        Ok(match find_window(self.window_id)? {
            None => SourceState::Destroyed,
            Some(window) if !window.is_on_screen() => SourceState::Hidden,
            Some(_) => SourceState::Available,
        })
    }
}

fn find_window(window_id: u32) -> Result<Option<SCWindow>> {
    let content = SCShareableContent::get().map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(content
        .windows()
        .into_iter()
        .find(|window| window.window_id() == window_id))
}

fn screenshot(window: &SCWindow, width: u32, height: u32) -> Result<Vec<u8>> {
    let filter = SCContentFilter::create().with_window(window).build();
    let configuration = SCStreamConfiguration::new()
        .with_width(width)
        .with_height(height)
        .with_shows_cursor(false);
    let image = SCScreenshotManager::capture_image(&filter, &configuration)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    image
        .rgba_data()
        .map_err(|error| anyhow::anyhow!(error.to_string()))
}

fn ensure_screen_capture_permission() -> Result<()> {
    ensure!(
        unsafe { CGPreflightScreenCaptureAccess() },
        "macOS screen recording permission is required"
    );
    Ok(())
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}
