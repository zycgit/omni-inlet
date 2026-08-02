use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturePermissionStatus {
    required: bool,
    pub(crate) granted: bool,
    settings_label: &'static str,
}

#[tauri::command]
pub fn capture_permission_status() -> CapturePermissionStatus {
    #[cfg(target_os = "macos")]
    let granted = unsafe { CGPreflightScreenCaptureAccess() };
    #[cfg(not(target_os = "macos"))]
    let granted = true;

    CapturePermissionStatus {
        required: cfg!(target_os = "macos"),
        granted,
        settings_label: "隐私与安全性 → 屏幕与系统录音",
    }
}

#[tauri::command]
pub fn request_capture_permission() -> bool {
    #[cfg(target_os = "macos")]
    return unsafe { CGRequestScreenCaptureAccess() };
    #[cfg(not(target_os = "macos"))]
    true
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}
