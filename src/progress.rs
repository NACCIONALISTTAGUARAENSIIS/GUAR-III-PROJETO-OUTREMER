#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use once_cell::sync::OnceCell;
use serde_json::json;
use tauri::{Emitter, WebviewWindow};

pub static MAIN_WINDOW: OnceCell<WebviewWindow> = OnceCell::new();

pub fn set_main_window(window: WebviewWindow) {
    MAIN_WINDOW.set(window).ok();
}

pub fn get_main_window() -> Option<&'static WebviewWindow> {
    MAIN_WINDOW.get()
}

/// This function checks if the program is running with a GUI window.
/// Returns `true` if a GUI window is initialized, `false` otherwise.
pub fn is_running_with_gui() -> bool {
    get_main_window().is_some()
}

/// ?? BESM-6 PIPELINE DOCTRINE ??
/// This code manages a multi-step process with a progress bar indicating the overall completion.
/// The progress updates are mapped to the new Out-of-Core Scanline pipeline:
///
/// [1/7] Fetching data... (Providers: OSM, LiDAR, CityGML, WFS) - Starts at: 0% / Completes at: 5%
/// [2/7] Parsing data... (Voxelizacao Local Determin�stica) - Starts at: 5% / Completes at: 15%
/// [3/7] Fetching elevation... (Virtual Grid & Downsampling) - Starts at: 15% / Completes at: 20%
/// [4/7] Building Global Constraints... (Highway Maps & Limits) - Starts at: 20% / Completes at: 25%
/// [5/7] Spatially Indexing Vectors... (R-Tree O(1) Routing) - Starts at: 25% / Completes at: 30%
/// [6/7] Sweeping Regions... (Out-of-Core Scanline & Core/Halo) - Starts at: 30% / Completes at: 90%
/// [7/7] Finalizing metadata & saving... (Episodic Flush) - Starts at: 90% / Completes at: 100%
///
/// The function `emit_gui_progress_update` is used to send real-time progress updates to the UI.
pub fn emit_gui_progress_update(progress: f64, message: &str) {
    if let Some(window) = get_main_window() {
        let payload = json!({
            "progress": progress,
            "message": message
        });

        if let Err(e) = window.emit("progress-update", payload) {
            let error_msg = format!("Failed to emit progress event: {}", e);
            eprintln!("{}", error_msg);
            #[cfg(feature = "gui")]
            send_log(LogLevel::Warning, &error_msg);
        }
    }
}

pub fn emit_gui_error(message: &str) {
    let truncated_message = if message.len() > 35 {
        &message[..35]
    } else {
        message
    };
    emit_gui_progress_update(0.0, &format!("Error! {truncated_message}"));
}

/// Emits an event when the world map preview is ready
pub fn emit_map_preview_ready() {
    if let Some(window) = get_main_window() {
        if let Err(e) = window.emit("map-preview-ready", ()) {
            eprintln!("Failed to emit map-preview-ready event: {}", e);
        }
    }
}

/// Emits an event to open the generated mcworld file
pub fn emit_open_mcworld_file(path: &str) {
    if let Some(window) = get_main_window() {
        if let Err(e) = window.emit("open-mcworld-file", path) {
            eprintln!("Failed to emit open-mcworld-file event: {}", e);
        }
    }
}