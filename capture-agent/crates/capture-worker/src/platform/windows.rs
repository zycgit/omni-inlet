use std::{ffi::c_void, fs, mem::size_of, path::Path, ptr::null_mut};

use anyhow::{Context, Result, bail, ensure};
use capture_protocol::{
    ApplicationInfo, CaptureSourceInfo, CaptureSourceKind, NativeTarget, WindowCandidate,
};
use windows::Win32::{
    Foundation::{CloseHandle, HWND, LPARAM, RECT},
    Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute},
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
        DeleteDC, DeleteObject, HGDIOBJ, SelectObject,
    },
    Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow},
    System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    },
    UI::WindowsAndMessaging::{
        EnumWindows, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow,
        IsWindowVisible, PW_RENDERFULLCONTENT,
    },
};
use windows::core::{BOOL, PWSTR};

use crate::source::{CaptureSource, CapturedFrame, SourceState};

pub fn enumerate_windows(thumbnail_directory: Option<&Path>) -> Result<Vec<WindowCandidate>> {
    if let Some(directory) = thumbnail_directory {
        fs::create_dir_all(directory)?;
    }
    let mut handles = Vec::<HWND>::new();
    unsafe extern "system" fn callback(hwnd: HWND, parameter: LPARAM) -> BOOL {
        let handles = unsafe { &mut *(parameter.0 as *mut Vec<HWND>) };
        handles.push(hwnd);
        BOOL(1)
    }
    unsafe {
        EnumWindows(
            Some(callback),
            LPARAM((&mut handles as *mut Vec<HWND>) as isize),
        )?;
    }

    let current_pid = std::process::id();
    let mut candidates = Vec::new();
    for hwnd in handles {
        let title = window_title(hwnd);
        if title.trim().is_empty() {
            continue;
        }
        let mut process_id = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };
        if process_id == current_pid {
            continue;
        }
        let Some((width, height)) = window_size(hwnd) else {
            continue;
        };
        let visible = is_window_presented(hwnd);
        let application_name = process_name(process_id).unwrap_or_else(|| "Windows 应用".into());
        let value = format!("0x{:X}", hwnd.0 as usize);
        let target = NativeTarget {
            kind: "windows-hwnd".into(),
            value: value.clone(),
        };
        let thumbnail_path = if visible {
            thumbnail_directory.and_then(|directory| {
                let path =
                    directory.join(format!("windows-{}.png", value.trim_start_matches("0x")));
                let frame = capture_hwnd(hwnd).ok()?;
                super::save_thumbnail(&frame.rgba, frame.width, frame.height, &path).ok()?;
                Some(path.to_string_lossy().into_owned())
            })
        } else {
            None
        };
        candidates.push(WindowCandidate {
            candidate_id: target.key(),
            application: ApplicationInfo {
                group_id: format!("windows:{}", application_name.to_ascii_lowercase()),
                display_name: application_name,
                process_id: Some(process_id),
                icon_path: None,
            },
            title,
            visible,
            capturable: visible,
            unavailable_reason: (!visible).then(|| "窗口已隐藏或最小化".into()),
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
    if kind != "windows-hwnd" {
        bail!("unsupported Windows target kind: {kind}");
    }
    let raw = usize::from_str_radix(value.trim_start_matches("0x"), 16)
        .with_context(|| format!("invalid HWND: {value}"))?;
    let hwnd = HWND(raw as *mut c_void);
    ensure!(
        unsafe { IsWindow(Some(hwnd)).as_bool() },
        "the target HWND does not exist"
    );
    Ok(Box::new(WindowsWindowSource::new(hwnd)?))
}

struct WindowsWindowSource {
    hwnd: HWND,
    info: CaptureSourceInfo,
}

impl WindowsWindowSource {
    fn new(hwnd: HWND) -> Result<Self> {
        let (width, height) =
            window_size(hwnd).context("the target window has no usable bounds")?;
        Ok(Self {
            hwnd,
            info: CaptureSourceInfo {
                kind: CaptureSourceKind::WindowsWindow,
                id: format!("0x{:X}", hwnd.0 as usize),
                title: window_title(hwnd),
                width,
                height,
                visible: is_window_presented(hwnd),
            },
        })
    }
}

impl CaptureSource for WindowsWindowSource {
    fn info(&self) -> CaptureSourceInfo {
        self.info.clone()
    }

    fn capture(&mut self) -> Result<CapturedFrame> {
        ensure!(
            self.state()? == SourceState::Available,
            "target window is not currently visible"
        );
        let frame = capture_hwnd(self.hwnd)?;
        self.info.width = frame.width;
        self.info.height = frame.height;
        Ok(frame)
    }

    fn state(&self) -> Result<SourceState> {
        unsafe {
            if !IsWindow(Some(self.hwnd)).as_bool() {
                Ok(SourceState::Destroyed)
            } else if !is_window_presented(self.hwnd) {
                Ok(SourceState::Hidden)
            } else {
                Ok(SourceState::Available)
            }
        }
    }
}

fn is_window_presented(hwnd: HWND) -> bool {
    if unsafe { !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() } {
        return false;
    }
    let mut cloaked = 0_u32;
    let result = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            size_of::<u32>() as u32,
        )
    };
    result.is_err() || cloaked == 0
}

fn window_title(hwnd: HWND) -> String {
    let mut buffer = vec![0_u16; 2048];
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

fn window_size(hwnd: HWND) -> Option<(u32, u32)> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect).ok()? };
    let width = rect.right.checked_sub(rect.left)?;
    let height = rect.bottom.checked_sub(rect.top)?;
    (width > 1 && height > 1).then_some((width as u32, height as u32))
}

fn process_name(process_id: u32) -> Option<String> {
    let process =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()? };
    let mut buffer = vec![0_u16; 32768];
    let mut length = buffer.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    };
    unsafe { CloseHandle(process).ok()? };
    result.ok()?;
    let path = String::from_utf16_lossy(&buffer[..length as usize]);
    Path::new(&path)
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())
}

fn capture_hwnd(hwnd: HWND) -> Result<CapturedFrame> {
    let (width, height) = window_size(hwnd).context("target window has invalid bounds")?;
    let memory_dc = unsafe { CreateCompatibleDC(None) };
    ensure!(!memory_dc.is_invalid(), "CreateCompatibleDC failed");
    let mut bits: *mut c_void = null_mut();
    let bitmap_info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let bitmap =
        unsafe { CreateDIBSection(None, &bitmap_info, DIB_RGB_COLORS, &mut bits, None, 0)? };
    let previous = unsafe { SelectObject(memory_dc, HGDIOBJ(bitmap.0)) };
    let printed =
        unsafe { PrintWindow(hwnd, memory_dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool() };
    let byte_length = width as usize * height as usize * 4;
    let mut rgba = vec![0_u8; byte_length];
    if printed && !bits.is_null() {
        let bgra = unsafe { std::slice::from_raw_parts(bits.cast::<u8>(), byte_length) };
        for (source, destination) in bgra.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
            destination[0] = source[2];
            destination[1] = source[1];
            destination[2] = source[0];
            destination[3] = 255;
        }
    }
    unsafe {
        SelectObject(memory_dc, previous);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(memory_dc);
    }
    ensure!(printed, "PrintWindow failed for the target HWND");
    Ok(CapturedFrame {
        width,
        height,
        rgba,
    })
}
