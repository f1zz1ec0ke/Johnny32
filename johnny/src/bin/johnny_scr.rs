//! The Windows screensaver shell.
//!
//! Build a `.scr` and Windows will run it with one of the standard arguments:
//!
//! * `/s` — full screen, the actual screensaver
//! * `/p <HWND>` — draw into the little preview pane in the settings dialog
//! * `/c[:<HWND>]` — show the configuration dialog
//!
//! Rename the resulting `johnny_scr.exe` to `Johnny.scr`, drop it in
//! `C:\Windows\System32`, and put `RESOURCE.001` and `RESOURCE.MAP` from your
//! own copy of the original beside it (or point `JC_DATA` at them).
//!
//! Everything below the shell is platform independent; this file is the only
//! Windows-specific part of the port.

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "johnny_scr is the Windows screensaver shell.\n\
         Build it with: cargo build --release --target x86_64-pc-windows-gnu\n\
         On other platforms use jcisland, which renders frames to PNG."
    );
}

#[cfg(windows)]
fn main() {
    windows_main();
}

#[cfg(windows)]
mod win {
    #![allow(non_snake_case, non_camel_case_types)]

    pub type HANDLE = *mut core::ffi::c_void;
    pub type HWND = HANDLE;
    pub type HDC = HANDLE;
    pub type HINSTANCE = HANDLE;
    pub type LRESULT = isize;
    pub type LPARAM = isize;
    pub type WPARAM = usize;

    pub const WS_POPUP: u32 = 0x8000_0000;
    pub const WS_VISIBLE: u32 = 0x1000_0000;
    pub const WS_CHILD: u32 = 0x4000_0000;
    pub const WS_EX_TOPMOST: u32 = 0x0000_0008;

    pub const WM_DESTROY: u32 = 0x0002;
    pub const WM_CLOSE: u32 = 0x0010;
    pub const WM_ERASEBKGND: u32 = 0x0014;
    pub const WM_PAINT: u32 = 0x000F;
    pub const WM_KEYDOWN: u32 = 0x0100;
    pub const WM_SYSKEYDOWN: u32 = 0x0104;
    pub const WM_MOUSEMOVE: u32 = 0x0200;
    pub const WM_LBUTTONDOWN: u32 = 0x0201;
    pub const WM_RBUTTONDOWN: u32 = 0x0204;
    pub const WM_MBUTTONDOWN: u32 = 0x0207;

    pub const PM_REMOVE: u32 = 0x0001;
    pub const SM_XVIRTUALSCREEN: i32 = 76;
    pub const SM_YVIRTUALSCREEN: i32 = 77;
    pub const SM_CXVIRTUALSCREEN: i32 = 78;
    pub const SM_CYVIRTUALSCREEN: i32 = 79;

    pub const DIB_RGB_COLORS: u32 = 0;
    pub const SRCCOPY: u32 = 0x00CC_0020;
    pub const BLACKNESS: u32 = 0x0000_0042;
    pub const COLORONCOLOR: i32 = 3;

    /// `MAKEINTRESOURCE(32512)`.
    pub const IDC_ARROW: usize = 32512;

    pub const MB_OK: u32 = 0x0000_0000;
    pub const MB_ICONINFORMATION: u32 = 0x0000_0040;

    #[repr(C)]
    pub struct POINT {
        pub x: i32,
        pub y: i32,
    }

    #[repr(C)]
    pub struct MSG {
        pub hwnd: HWND,
        pub message: u32,
        pub wParam: WPARAM,
        pub lParam: LPARAM,
        pub time: u32,
        pub pt: POINT,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    pub struct RECT {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }

    #[repr(C)]
    pub struct WNDCLASSW {
        pub style: u32,
        pub lpfnWndProc: Option<unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>,
        pub cbClsExtra: i32,
        pub cbWndExtra: i32,
        pub hInstance: HINSTANCE,
        pub hIcon: HANDLE,
        pub hCursor: HANDLE,
        pub hbrBackground: HANDLE,
        pub lpszMenuName: *const u16,
        pub lpszClassName: *const u16,
    }

    #[repr(C)]
    pub struct BITMAPINFOHEADER {
        pub biSize: u32,
        pub biWidth: i32,
        pub biHeight: i32,
        pub biPlanes: u16,
        pub biBitCount: u16,
        pub biCompression: u32,
        pub biSizeImage: u32,
        pub biXPelsPerMeter: i32,
        pub biYPelsPerMeter: i32,
        pub biClrUsed: u32,
        pub biClrImportant: u32,
    }

    /// A BITMAPINFO with its 256-entry palette inline.
    #[repr(C)]
    pub struct BITMAPINFO256 {
        pub header: BITMAPINFOHEADER,
        pub colors: [u32; 256],
    }

    #[link(name = "user32")]
    extern "system" {
        pub fn RegisterClassW(class: *const WNDCLASSW) -> u16;
        pub fn CreateWindowExW(
            ex_style: u32,
            class: *const u16,
            window: *const u16,
            style: u32,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            parent: HWND,
            menu: HANDLE,
            instance: HINSTANCE,
            param: *mut core::ffi::c_void,
        ) -> HWND;
        pub fn DefWindowProcW(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT;
        pub fn PostQuitMessage(code: i32);
        pub fn DestroyWindow(hwnd: HWND) -> i32;
        pub fn PeekMessageW(msg: *mut MSG, hwnd: HWND, min: u32, max: u32, remove: u32) -> i32;
        pub fn TranslateMessage(msg: *const MSG) -> i32;
        pub fn DispatchMessageW(msg: *const MSG) -> LRESULT;
        pub fn GetDC(hwnd: HWND) -> HDC;
        pub fn ReleaseDC(hwnd: HWND, dc: HDC) -> i32;
        pub fn GetClientRect(hwnd: HWND, rect: *mut RECT) -> i32;
        pub fn GetSystemMetrics(index: i32) -> i32;
        pub fn ShowCursor(show: i32) -> i32;
        pub fn GetCursorPos(p: *mut POINT) -> i32;
        pub fn MessageBoxW(hwnd: HWND, text: *const u16, caption: *const u16, kind: u32) -> i32;
        pub fn LoadCursorW(instance: HINSTANCE, name: *const u16) -> HANDLE;
        pub fn SetForegroundWindow(hwnd: HWND) -> i32;
        pub fn SetFocus(hwnd: HWND) -> HWND;
        pub fn IsWindow(hwnd: HWND) -> i32;
        pub fn EnumDisplayMonitors(
            dc: HDC,
            clip: *const RECT,
            proc: MonitorEnumProc,
            data: isize,
        ) -> i32;
    }

    pub type HMONITOR = *mut core::ffi::c_void;
    pub type MonitorEnumProc =
        unsafe extern "system" fn(HMONITOR, HDC, *mut RECT, isize) -> i32;

    #[link(name = "gdi32")]
    extern "system" {
        pub fn StretchDIBits(
            dc: HDC,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            sx: i32,
            sy: i32,
            sw: i32,
            sh: i32,
            bits: *const core::ffi::c_void,
            info: *const BITMAPINFO256,
            usage: u32,
            rop: u32,
        ) -> i32;
        pub fn SetStretchBltMode(dc: HDC, mode: i32) -> i32;
        pub fn PatBlt(dc: HDC, x: i32, y: i32, w: i32, h: i32, rop: u32) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetModuleHandleW(name: *const u16) -> HINSTANCE;
        pub fn GetTickCount() -> u32;
        pub fn Sleep(ms: u32);
    }

    pub fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(windows)]
fn windows_main() {
    use johnny::{dgds::Archive, director::Director};
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
    use win::*;

    /// Collect one monitor's rectangle. `EnumDisplayMonitors` hands the callback
    /// an `LPARAM` of our choosing, which is where the `Vec` comes from.
    unsafe extern "system" fn collect_monitor(
        _monitor: HMONITOR,
        _dc: HDC,
        rect: *mut RECT,
        data: isize,
    ) -> i32 {
        if !rect.is_null() && data != 0 {
            let out = &mut *(data as *mut Vec<RECT>);
            out.push(*rect);
        }
        1
    }

    /// The largest `sw` x `sh` rectangle that fits inside `dst`, centred, with the
    /// proportions kept. Three monitors then get three islands rather than one
    /// island smeared across all three.
    fn fit(dst: RECT, sw: i32, sh: i32) -> (i32, i32, i32, i32) {
        let (w, h) = (dst.right - dst.left, dst.bottom - dst.top);
        if w <= 0 || h <= 0 || sw <= 0 || sh <= 0 {
            return (dst.left, dst.top, w.max(0), h.max(0));
        }
        // Widest first: if the full width is too tall to fit, go by height.
        let by_width = w as i64 * sh as i64 / sw as i64;
        if by_width <= h as i64 {
            let fh = by_width as i32;
            (dst.left, dst.top + (h - fh) / 2, w, fh)
        } else {
            let fw = (h as i64 * sw as i64 / sh as i64) as i32;
            (dst.left + (w - fw) / 2, dst.top, fw, h)
        }
    }

    /// Set by the window procedure when the user wants out.
    static QUIT: AtomicBool = AtomicBool::new(false);
    /// Where the cursor was when we started, so that the spurious
    /// WM_MOUSEMOVE Windows sends as the window appears under the pointer
    /// does not dismiss the saver instantly.
    static START_X: AtomicI32 = AtomicI32::new(0);
    static START_Y: AtomicI32 = AtomicI32::new(0);
    static IS_PREVIEW: AtomicBool = AtomicBool::new(false);
    /// The letterbox around each island needs painting. Only when something
    /// has damaged it: painting it every frame would flicker, since the blit
    /// that follows covers the island but not the bars.
    static LETTERBOX: AtomicBool = AtomicBool::new(true);

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        match msg {
            WM_ERASEBKGND => 1, // we paint every frame; do not flicker
            WM_PAINT => {
                // Something uncovered us. The next frame repaints the island
                // anyway; the bars around it have to be asked for.
                LETTERBOX.store(true, Ordering::Relaxed);
                DefWindowProcW(hwnd, msg, w, l)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            WM_CLOSE => {
                QUIT.store(true, Ordering::Relaxed);
                0
            }
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
                if !IS_PREVIEW.load(Ordering::Relaxed) {
                    QUIT.store(true, Ordering::Relaxed);
                }
                0
            }
            WM_MOUSEMOVE => {
                if !IS_PREVIEW.load(Ordering::Relaxed) {
                    let mut p = POINT { x: 0, y: 0 };
                    GetCursorPos(&mut p);
                    let dx = p.x - START_X.load(Ordering::Relaxed);
                    let dy = p.y - START_Y.load(Ordering::Relaxed);
                    if dx * dx + dy * dy > 16 {
                        QUIT.store(true, Ordering::Relaxed);
                    }
                }
                0
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }

    // ---- arguments -------------------------------------------------------
    #[derive(PartialEq)]
    enum Mode {
        Saver,
        Preview(HWND),
        Config,
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let joined = args.join(" ");
    let flag = args
        .first()
        .map(|a| a.trim_start_matches(['/', '-']).to_ascii_lowercase())
        .unwrap_or_default();
    let parent_hwnd = |s: &str| -> HWND {
        // The handle may arrive as "/p 12345", "/p:12345" or in hex.
        let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
        digits.parse::<usize>().unwrap_or(0) as HWND
    };
    let mode = if flag.starts_with('p') {
        let rest = if flag.len() > 1 {
            flag[1..].to_string()
        } else {
            args.get(1).cloned().unwrap_or_default()
        };
        let parent = parent_hwnd(&rest);
        if parent.is_null() {
            // A preview request we cannot honour. Falling through to the
            // full-screen arm here would blank the whole desktop from the
            // settings dialog, which is a memorable way to fail.
            return;
        }
        Mode::Preview(parent)
    } else if flag.starts_with('c') || flag.is_empty() {
        Mode::Config
    } else {
        let _ = joined;
        Mode::Saver
    };

    if mode == Mode::Config {
        let text = wide(
            "Johnny Castaway\r\n\r\n\
             A native reimplementation of the 1992 Dynamix screensaver, built \
             from a disassembly of the original.\r\n\r\n\
             Place RESOURCE.001 and RESOURCE.MAP from your own copy of the \
             original beside this screensaver, or set JC_DATA to the folder \
             holding them.\r\n\r\n\
             There is nothing to configure yet.",
        );
        let caption = wide("Johnny Castaway");
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                caption.as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            );
        }
        return;
    }

    // ---- data ------------------------------------------------------------
    // Look beside the executable, not in the working directory. Windows runs
    // a .scr from System32 with a working directory of its own choosing, so
    // "put the data files next to it" only means anything if we resolve
    // against the binary's own path.
    let dir = std::env::var("JC_DATA").unwrap_or_else(|_| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_string_lossy().into_owned()))
            .unwrap_or_else(|| ".".into())
    });
    let data = match std::fs::read(format!("{dir}/RESOURCE.001")) {
        Ok(d) => d,
        Err(e) => {
            let text = wide(&format!(
                "Could not open RESOURCE.001 in {dir}\r\n\r\n{e}\r\n\r\n\
                 Copy RESOURCE.001 and RESOURCE.MAP from your own copy of \
                 Johnny Castaway next to this screensaver, or set JC_DATA."
            ));
            let caption = wide("Johnny Castaway");
            unsafe {
                MessageBoxW(
                    std::ptr::null_mut(),
                    text.as_ptr(),
                    caption.as_ptr(),
                    MB_OK | MB_ICONINFORMATION,
                );
            }
            return;
        }
    };
    let map = std::fs::read(format!("{dir}/RESOURCE.MAP")).ok();
    let Ok(archive) = Archive::load(&data, map.as_deref()) else {
        return;
    };

    // ---- window ----------------------------------------------------------
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let class_name = wide("JohnnyCastawaySaver");
        // IDC_ARROW is MAKEINTRESOURCE(32512): the integer cast to a
        // pointer, not a string containing that code point.
        let arrow = IDC_ARROW as *const u16;
        let class = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), arrow),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        RegisterClassW(&class);

        let (hwnd, preview) = match mode {
            Mode::Preview(parent) => {
                IS_PREVIEW.store(true, Ordering::Relaxed);
                let mut r = RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };
                GetClientRect(parent, &mut r);
                let h = CreateWindowExW(
                    0,
                    class_name.as_ptr(),
                    class_name.as_ptr(),
                    WS_CHILD | WS_VISIBLE,
                    0,
                    0,
                    r.right - r.left,
                    r.bottom - r.top,
                    parent,
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null_mut(),
                );
                (h, true)
            }
            _ => {
                // Cover the whole virtual desktop, so multiple monitors are
                // one continuous island rather than one saver per screen.
                let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
                let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
                let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
                let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
                let mut start = POINT { x: 0, y: 0 };
                GetCursorPos(&mut start);
                START_X.store(start.x, Ordering::Relaxed);
                START_Y.store(start.y, Ordering::Relaxed);
                ShowCursor(0);
                let h = CreateWindowExW(
                    WS_EX_TOPMOST,
                    class_name.as_ptr(),
                    class_name.as_ptr(),
                    WS_POPUP | WS_VISIBLE,
                    x,
                    y,
                    w,
                    h,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null_mut(),
                );
                (h, false)
            }
        };
        if hwnd.is_null() {
            return;
        }
        if !preview {
            // Without the foreground, a top-most popup receives no keyboard
            // input and a key press would not dismiss the saver.
            SetForegroundWindow(hwnd);
            SetFocus(hwnd);
        }

        // Where to draw. The window spans the whole virtual desktop, so
        // stretching one surface across it distorts the island and splits it
        // over the bezels. Draw one copy per monitor instead, in that
        // monitor's own client-relative rectangle.
        let mut panes: Vec<RECT> = Vec::new();
        if !preview {
            let origin = (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
            );
            let mut screens: Vec<RECT> = Vec::new();
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                collect_monitor,
                &mut screens as *mut _ as isize,
            );
            panes = screens
                .into_iter()
                .map(|m| RECT {
                    left: m.left - origin.0,
                    top: m.top - origin.1,
                    right: m.right - origin.0,
                    bottom: m.bottom - origin.1,
                })
                .collect();
            // Black it out now rather than on the first drawn frame, so the
            // desktop does not show through the moment the window appears.
            let dc = GetDC(hwnd);
            PatBlt(
                dc,
                0,
                0,
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
                BLACKNESS,
            );
            ReleaseDC(hwnd, dc);
        }

        // ---- engine ------------------------------------------------------
        let seed = GetTickCount() as u64;
        let mut director = Director::new(seed);
        let mut info = BITMAPINFO256 {
            header: BITMAPINFOHEADER {
                biSize: core::mem::size_of::<BITMAPINFOHEADER>() as u32,
                // DIB rows are DWORD-aligned. At 8bpp that needs the width to
                // be a multiple of 4; 640 is, so the surface can be handed
                // over without a stride fixup.
                biWidth: johnny::render::SCREEN_W as i32,
                // Negative height: the surface is top-down.
                biHeight: -(johnny::render::SCREEN_H as i32),
                biPlanes: 1,
                biBitCount: 8,
                biCompression: 0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 256,
                biClrImportant: 256,
            },
            colors: [0; 256],
        };

        /// One tick is 16ms; see `director::MS_PER_TICK`.
        const MS_PER_TICK: u32 = johnny::director::MS_PER_TICK;
        let mut last = GetTickCount();
        let mut msg: MSG = core::mem::zeroed();

        while !QUIT.load(Ordering::Relaxed) {
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if QUIT.load(Ordering::Relaxed) {
                break;
            }
            // The settings dialog closes without telling us; if the parent
            // pane has gone, so should we, or preview processes pile up.
            if let Mode::Preview(parent) = mode {
                if IsWindow(parent) == 0 {
                    break;
                }
            }

            // Catch up on however many ticks the clock has moved, the way the
            // original's dispatcher does, but bounded so a hitch cannot make
            // it sprint.
            let now = GetTickCount();
            let mut due = ((now.wrapping_sub(last)) / MS_PER_TICK).min(8);
            if due == 0 {
                Sleep(1);
                continue;
            }
            last = last.wrapping_add(due * MS_PER_TICK);
            let mut drew = false;
            while due > 0 {
                drew |= director.tick(&archive);
                due -= 1;
            }
            if !drew {
                continue;
            }

            for (i, c) in director.stage.palette().iter().enumerate() {
                // RGBQUAD is blue, green, red, reserved.
                info.colors[i] =
                    (c[2] as u32) | ((c[1] as u32) << 8) | ((c[0] as u32) << 16);
            }

            let dc = GetDC(hwnd);
            let mut client = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            GetClientRect(hwnd, &mut client);
            if LETTERBOX.swap(false, Ordering::Relaxed) {
                PatBlt(
                    dc,
                    0,
                    0,
                    client.right - client.left,
                    client.bottom - client.top,
                    BLACKNESS,
                );
            }
            SetStretchBltMode(dc, COLORONCOLOR);
            let (sw, sh) = (
                johnny::render::SCREEN_W as i32,
                johnny::render::SCREEN_H as i32,
            );
            // No monitors enumerated (or a preview pane): fall back to the
            // client area, still keeping the proportions.
            let targets: &[RECT] = if panes.is_empty() {
                std::slice::from_ref(&client)
            } else {
                &panes
            };
            for pane in targets {
                let (x, y, w, h) = fit(*pane, sw, sh);
                StretchDIBits(
                    dc,
                    x,
                    y,
                    w,
                    h,
                    0,
                    0,
                    sw,
                    sh,
                    director.stage.surface.pixels.as_ptr() as *const _,
                    &info,
                    DIB_RGB_COLORS,
                    SRCCOPY,
                );
            }
            ReleaseDC(hwnd, dc);
        }

        if !preview {
            ShowCursor(1);
        }
        DestroyWindow(hwnd);
    }
}
