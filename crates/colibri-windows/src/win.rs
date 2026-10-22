use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use colibri_core::rig::{MeasureError, PatchDisplay, Rgb};
use windows_sys::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_DEVICE_INFO_TYPE, DISPLAYCONFIG_MODE_INFO,
    DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME,
    QDC_ONLY_ACTIVE_PATHS,
};
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_CLASS_ALREADY_EXISTS, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, HWND,
    LPARAM, LRESULT, LUID, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Dwm::DwmFlush;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateDCW, CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint,
    EnumDisplayMonitors, FillRect, GetMonitorInfoW, InvalidateRect, SetBkMode, SetTextColor,
    DT_BOTTOM, DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    PAINTSTRUCT, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::ColorSystem::{GetDeviceGammaRamp, SetDeviceGammaRamp};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW,
    GetWindowLongPtrW, PostMessageW, PostQuitMessage, RegisterClassW, SetCursor,
    SetForegroundWindow, SetWindowLongPtrW, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
    GWLP_USERDATA, MONITORINFOF_PRIMARY, MSG, SW_SHOW, WM_APP, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_PAINT, WM_SETCURSOR, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};

use crate::{ColorMode, DisplaySource, Error, GammaRamp, Monitor};

const WM_SHOW_PATCH: u32 = WM_APP + 1;
const WM_SET_STATUS: u32 = WM_APP + 2;
const VK_ESCAPE: usize = 0x1B;
const VK_SPACE: usize = 0x20;

/// The status text is gray, so it adds little light to a dark patch.
const STATUS_COLOR: u32 = 0x77_77_77;
/// Gap between the status text and the window edges, in pixels.
const STATUS_MARGIN: i32 = 16;
/// Height of the status text line, in pixels. Enough for the default font.
const STATUS_HEIGHT: i32 = 24;

/// `DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO_2`, new in Windows 11 24H2 and
/// missing from windows-sys 0.61.2. The bit fields of its union are kept as
/// one `u32`. See:
/// https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-displayconfig_get_advanced_color_info_2
#[repr(C)]
struct AdvancedColorInfo2 {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    flags: u32,
    color_encoding: i32,
    bits_per_color_channel: u32,
    active_color_mode: i32,
}

const DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO_2: DISPLAYCONFIG_DEVICE_INFO_TYPE = 15;

// DISPLAYCONFIG_ADVANCED_COLOR_MODE values. WCG is what the Settings app
// calls Auto Color Management.
const ADVANCED_COLOR_MODE_SDR: i32 = 0;
const ADVANCED_COLOR_MODE_WCG: i32 = 1;
const ADVANCED_COLOR_MODE_HDR: i32 = 2;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The text of a nul-terminated UTF-16 buffer.
fn from_wide(s: &[u16]) -> String {
    let len = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..len])
}

fn last_error(call: &'static str) -> Error {
    Error::Os(call, unsafe { GetLastError() })
}

/// All monitors attached to the desktop.
pub fn monitors() -> Result<Vec<Monitor>, Error> {
    unsafe extern "system" fn collect(
        handle: HMONITOR,
        _dc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> windows_sys::core::BOOL {
        let list = &mut *(data as *mut Vec<Monitor>);
        let mut info = MONITORINFOEXW {
            monitorInfo: MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
                ..Default::default()
            },
            ..Default::default()
        };
        if GetMonitorInfoW(handle, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) != 0 {
            let r = info.monitorInfo.rcMonitor;
            let device = from_wide(&info.szDevice);
            list.push(Monitor {
                index: list.len(),
                name: device.clone(),
                source: None,
                device,
                left: r.left,
                top: r.top,
                width: r.right - r.left,
                height: r.bottom - r.top,
                primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
        1
    }

    let mut list: Vec<Monitor> = Vec::new();
    let ok = unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect),
            &mut list as *mut Vec<Monitor> as LPARAM,
        )
    };
    if ok == 0 {
        return Err(last_error("EnumDisplayMonitors"));
    }

    // Without the display configuration, monitors keep their GDI name and
    // no source. The patch window and gamma ramp work without them.
    let configured = configured_displays().unwrap_or_default();
    for monitor in &mut list {
        let Some(display) = configured.iter().find(|d| d.device == monitor.device) else {
            continue;
        };
        if !display.name.is_empty() {
            monitor.name = display.name.clone();
        }
        monitor.source = Some(display.source);
    }
    Ok(list)
}

/// An active display path, as `QueryDisplayConfig` describes it.
struct ConfiguredDisplay {
    /// GDI device name of the path's source, to match a [`Monitor`].
    device: String,
    /// The monitor's friendly name. Empty for some built-in panels.
    name: String,
    source: DisplaySource,
}

fn configured_displays() -> Result<Vec<ConfiguredDisplay>, Error> {
    let paths = active_paths()?;
    let mut displays = Vec::new();
    for path in &paths {
        let source = &path.sourceInfo;
        let target = &path.targetInfo;
        let Some(device) = source_device_name(source.adapterId, source.id) else {
            continue;
        };
        displays.push(ConfiguredDisplay {
            device,
            name: target_friendly_name(target.adapterId, target.id).unwrap_or_default(),
            source: DisplaySource {
                adapter_low: target.adapterId.LowPart,
                adapter_high: target.adapterId.HighPart,
                id: source.id,
                target_id: target.id,
            },
        });
    }
    Ok(displays)
}

/// The active paths of the display configuration. The configuration can
/// change between sizing the buffers and filling them, so it tries again
/// a few times.
fn active_paths() -> Result<Vec<DISPLAYCONFIG_PATH_INFO>, Error> {
    let mut status = ERROR_INSUFFICIENT_BUFFER;
    for _ in 0..3 {
        let (mut path_count, mut mode_count) = (0u32, 0u32);
        status = unsafe {
            GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
        };
        if status != ERROR_SUCCESS {
            return Err(Error::Os("GetDisplayConfigBufferSizes", status));
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        status = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                std::ptr::null_mut(),
            )
        };
        if status == ERROR_SUCCESS {
            paths.truncate(path_count as usize);
            return Ok(paths);
        }
        if status != ERROR_INSUFFICIENT_BUFFER {
            break;
        }
    }
    Err(Error::Os("QueryDisplayConfig", status))
}

fn device_info_header(
    info_type: DISPLAYCONFIG_DEVICE_INFO_TYPE,
    size: usize,
    adapter: LUID,
    id: u32,
) -> DISPLAYCONFIG_DEVICE_INFO_HEADER {
    DISPLAYCONFIG_DEVICE_INFO_HEADER {
        r#type: info_type,
        size: size as u32,
        adapterId: adapter,
        id,
    }
}

/// The GDI name, such as `\\.\DISPLAY1`, of a display configuration source.
fn source_device_name(adapter: LUID, id: u32) -> Option<String> {
    let mut request = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
        header: device_info_header(
            DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
            std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>(),
            adapter,
            id,
        ),
        ..Default::default()
    };
    let status = unsafe {
        DisplayConfigGetDeviceInfo(&mut request as *mut _ as *mut DISPLAYCONFIG_DEVICE_INFO_HEADER)
    };
    if status != 0 {
        return None;
    }
    Some(from_wide(&request.viewGdiDeviceName))
}

/// The monitor's friendly name, such as `ASUS PA328CGV`, from its EDID.
fn target_friendly_name(adapter: LUID, id: u32) -> Option<String> {
    let mut request = DISPLAYCONFIG_TARGET_DEVICE_NAME {
        header: device_info_header(
            DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
            std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>(),
            adapter,
            id,
        ),
        ..Default::default()
    };
    let status = unsafe {
        DisplayConfigGetDeviceInfo(&mut request as *mut _ as *mut DISPLAYCONFIG_DEVICE_INFO_HEADER)
    };
    if status != 0 {
        return None;
    }
    Some(from_wide(&request.monitorFriendlyDeviceName))
}

fn adapter_luid(source: &DisplaySource) -> LUID {
    LUID {
        LowPart: source.adapter_low,
        HighPart: source.adapter_high,
    }
}

/// Whether Windows shows `monitor` in SDR, with Auto Color Management, or
/// in HDR. Before Windows 11 24H2 the query fails, and the answer is
/// [`ColorMode::Unknown`] (**? to be validated on Windows**).
pub fn color_mode(monitor: &Monitor) -> Result<ColorMode, Error> {
    let Some(source) = monitor.source else {
        return Err(Error::UnknownDisplay);
    };
    let mut info = AdvancedColorInfo2 {
        header: device_info_header(
            DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO_2,
            std::mem::size_of::<AdvancedColorInfo2>(),
            adapter_luid(&source),
            source.target_id,
        ),
        flags: 0,
        color_encoding: 0,
        bits_per_color_channel: 0,
        active_color_mode: 0,
    };
    let status = unsafe {
        DisplayConfigGetDeviceInfo(&mut info as *mut _ as *mut DISPLAYCONFIG_DEVICE_INFO_HEADER)
    };
    if status != 0 {
        return Ok(ColorMode::Unknown);
    }
    let mode = match info.active_color_mode {
        ADVANCED_COLOR_MODE_SDR => ColorMode::Sdr,
        ADVANCED_COLOR_MODE_WCG => ColorMode::AutoColorManagement,
        ADVANCED_COLOR_MODE_HDR => ColorMode::Hdr,
        _ => ColorMode::Unknown,
    };
    Ok(mode)
}

fn with_monitor_dc<T>(
    monitor: &Monitor,
    f: impl FnOnce(HDC) -> Result<T, Error>,
) -> Result<T, Error> {
    let device = wide(&monitor.device);
    let dc = unsafe {
        CreateDCW(
            device.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if dc.is_null() {
        return Err(last_error("CreateDCW"));
    }
    let result = f(dc);
    unsafe { DeleteDC(dc) };
    result
}

/// The gamma ramp currently loaded for `monitor`.
pub fn gamma_ramp(monitor: &Monitor) -> Result<GammaRamp, Error> {
    with_monitor_dc(monitor, |dc| {
        let mut ramp: GammaRamp = [[0; 256]; 3];
        if unsafe { GetDeviceGammaRamp(dc, ramp.as_mut_ptr() as *mut _) } == 0 {
            return Err(last_error("GetDeviceGammaRamp"));
        }
        Ok(ramp)
    })
}

/// Load a gamma ramp for `monitor`. Windows refuses ramps far from identity
/// unless the system allows a wider range.
pub fn set_gamma_ramp(monitor: &Monitor, ramp: &GammaRamp) -> Result<(), Error> {
    with_monitor_dc(monitor, |dc| {
        if unsafe { SetDeviceGammaRamp(dc, ramp.as_ptr() as *const _) } == 0 {
            return Err(last_error("SetDeviceGammaRamp"));
        }
        Ok(())
    })
}

/// State shared between the window procedure and its owner.
struct WindowState {
    color: u32,
    /// Side of the centred patch as a fraction of the monitor's shorter side.
    size: f64,
    awaiting_paint: bool,
    painted: Sender<()>,
    cancelled: Arc<AtomicBool>,
    started: Arc<AtomicBool>,
    status: Arc<Mutex<String>>,
}

/// A borderless, topmost window covering one monitor: black, with a centred
/// square patch. Patches are 8-bit and drawn with GDI, which Windows does
/// not colour-manage (Auto Color Management aside).
pub struct PatchWindow {
    hwnd: usize,
    thread: Option<JoinHandle<()>>,
    painted: Receiver<()>,
    cancelled: Arc<AtomicBool>,
    started: Arc<AtomicBool>,
    status: Arc<Mutex<String>>,
}

/// Writes the patch window's corner status text, from any thread.
#[derive(Clone)]
pub struct StatusLine {
    hwnd: usize,
    text: Arc<Mutex<String>>,
}

impl StatusLine {
    pub fn set(&self, text: &str) {
        if let Ok(mut current) = self.text.lock() {
            *current = text.to_string();
        }
        unsafe { PostMessageW(self.hwnd as HWND, WM_SET_STATUS, 0, 0) };
    }
}

impl PatchWindow {
    /// Open the window on `monitor`. `size` is the patch side as a fraction
    /// of the monitor's shorter side (1.0 fills the height).
    pub fn open(monitor: &Monitor, size: f64) -> Result<PatchWindow, Error> {
        let (painted_tx, painted_rx) = channel();
        let (hwnd_tx, hwnd_rx) = channel::<Result<usize, Error>>();
        let cancelled = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(String::new()));
        let state = WindowState {
            color: 0,
            size: size.clamp(0.05, 1.0),
            awaiting_paint: false,
            painted: painted_tx,
            cancelled: cancelled.clone(),
            started: started.clone(),
            status: status.clone(),
        };
        let monitor = monitor.clone();
        let thread = std::thread::spawn(move || unsafe { run_window(monitor, state, hwnd_tx) });
        let hwnd = hwnd_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| Error::Timeout)??;
        Ok(PatchWindow {
            hwnd,
            thread: Some(thread),
            painted: painted_rx,
            cancelled,
            started,
            status,
        })
    }

    /// Show an 8-bit colour and wait until it has been composited.
    pub fn show_rgb8(&mut self, rgb: [u8; 3]) -> Result<(), Error> {
        let color = rgb[0] as u32 | (rgb[1] as u32) << 8 | (rgb[2] as u32) << 16;
        while self.painted.try_recv().is_ok() {}
        let ok = unsafe { PostMessageW(self.hwnd as HWND, WM_SHOW_PATCH, color as WPARAM, 0) };
        if ok == 0 {
            return Err(last_error("PostMessageW"));
        }
        self.painted
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| Error::Timeout)
    }

    /// True once the user has pressed Escape in the window.
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub fn status_line(&self) -> StatusLine {
        StatusLine {
            hwnd: self.hwnd,
            text: self.status.clone(),
        }
    }

    /// Shows `prompt` in the status line and blocks until Space or a click
    /// in the window (Ok) or Esc (Err(MeasureError::Cancelled)).
    ///
    /// A click starts too, because Windows doesn't always let a console
    /// program bring its window to the front. Space then goes to the
    /// terminal, while a click always lands on the patch window.
    pub fn wait_for_start(&self, prompt: &str) -> Result<(), MeasureError> {
        let status = self.status_line();
        self.started.store(false, Ordering::Relaxed);
        status.set(prompt);
        loop {
            if self.cancelled() {
                return Err(MeasureError::Cancelled);
            }
            if self.started.load(Ordering::Relaxed) {
                status.set("");
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl PatchDisplay for PatchWindow {
    fn show(&mut self, patch: Rgb) -> Result<(), MeasureError> {
        if self.cancelled() {
            return Err(MeasureError::Cancelled);
        }
        let rgb = patch.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
        self.show_rgb8(rgb)
            .map_err(|e| MeasureError::Instrument(e.to_string()))
    }

    fn bits(&self) -> u32 {
        8
    }
}

impl Drop for PatchWindow {
    fn drop(&mut self) {
        unsafe { PostMessageW(self.hwnd as HWND, WM_CLOSE, 0, 0) };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

unsafe fn run_window(monitor: Monitor, state: WindowState, hwnd_tx: Sender<Result<usize, Error>>) {
    let class = wide("colibri-patch-window");
    let instance = GetModuleHandleW(std::ptr::null());
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: class.as_ptr(),
        ..std::mem::zeroed()
    };
    if RegisterClassW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
        let _ = hwnd_tx.send(Err(last_error("RegisterClassW")));
        return;
    }
    let title = wide("colibri patch");
    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        class.as_ptr(),
        title.as_ptr(),
        WS_POPUP,
        monitor.left,
        monitor.top,
        monitor.width,
        monitor.height,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        instance,
        std::ptr::null(),
    );
    if hwnd.is_null() {
        let _ = hwnd_tx.send(Err(last_error("CreateWindowExW")));
        return;
    }
    let state = Box::into_raw(Box::new(state));
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
    let _ = hwnd_tx.send(Ok(hwnd as usize));

    let mut msg: MSG = std::mem::zeroed();
    while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    drop(Box::from_raw(state));
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
    if state.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let state = &mut *state;
    match msg {
        WM_SHOW_PATCH => {
            state.color = wparam as u32;
            state.awaiting_paint = true;
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_SET_STATUS => {
            // CAUTION! Invalidating the whole window repaints the patch
            // through black, which could land in the middle of a reading.
            // Only the text's rectangle is repainted, and it never overlaps
            // the patch.
            let mut client: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut client);
            let text_rect = status_rect(&client, &patch_rect(&client, state.size));
            InvalidateRect(hwnd, &text_rect, 0);
            0
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut client: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut client);
            let black = CreateSolidBrush(0);
            FillRect(dc, &client, black);
            DeleteObject(black);
            let patch = patch_rect(&client, state.size);
            let brush = CreateSolidBrush(state.color);
            FillRect(dc, &patch, brush);
            DeleteObject(brush);
            let text = state.status.lock().map(|t| t.clone()).unwrap_or_default();
            if !text.is_empty() {
                let mut text_rect = status_rect(&client, &patch);
                let text = wide(&text);
                SetTextColor(dc, STATUS_COLOR);
                SetBkMode(dc, TRANSPARENT as i32);
                // The length excludes the trailing nul added by `wide`.
                DrawTextW(
                    dc,
                    text.as_ptr(),
                    text.len() as i32 - 1,
                    &mut text_rect,
                    DT_LEFT | DT_BOTTOM | DT_SINGLELINE | DT_END_ELLIPSIS,
                );
            }
            EndPaint(hwnd, &ps);
            if state.awaiting_paint {
                // Wait for the compositor to present the new frame.
                DwmFlush();
                DwmFlush();
                state.awaiting_paint = false;
                let _ = state.painted.send(());
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_SETCURSOR => {
            SetCursor(std::ptr::null_mut());
            1
        }
        WM_KEYDOWN if wparam == VK_ESCAPE => {
            state.cancelled.store(true, Ordering::Relaxed);
            0
        }
        WM_KEYDOWN if wparam == VK_SPACE => {
            state.started.store(true, Ordering::Relaxed);
            0
        }
        WM_LBUTTONDOWN => {
            state.started.store(true, Ordering::Relaxed);
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// The centred square patch, `size` being its side as a fraction of the
/// client area's shorter side.
fn patch_rect(client: &RECT, size: f64) -> RECT {
    let (w, h) = (client.right - client.left, client.bottom - client.top);
    let side = (w.min(h) as f64 * size).round() as i32;
    RECT {
        left: (w - side) / 2,
        top: (h - side) / 2,
        right: (w + side) / 2,
        bottom: (h + side) / 2,
    }
}

/// The status text line in the bottom-left corner. When it is level with
/// the patch, it stops short of the patch's left edge, and can end up empty.
fn status_rect(client: &RECT, patch: &RECT) -> RECT {
    let bottom = client.bottom - STATUS_MARGIN;
    let top = bottom - STATUS_HEIGHT;
    let mut right = client.right - STATUS_MARGIN;
    let is_level_with_patch = top < patch.bottom && bottom > patch.top;
    if is_level_with_patch {
        right = right.min(patch.left - STATUS_MARGIN);
    }
    RECT {
        left: STATUS_MARGIN,
        top,
        right: right.max(STATUS_MARGIN),
        bottom,
    }
}
