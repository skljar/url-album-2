/// Custom Win32 platform for Slint 1.16 — Windows 7+ compatible, no WinRT/api-ms-win-core-winrt.
/// Replaces winit backend to remove Win8+ API dependencies.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::Sleep;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::Memory::*;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PlatformError, WindowAdapter, WindowEvent, PointerEventButton, Key};
use slint::{LogicalPosition, PhysicalSize, WindowSize};

// ── Event loop proxy (для invoke_from_event_loop из фоновых потоков) ─────────

const WM_SLINT_WAKE: u32 = WM_APP + 1;

type CallbackQueue = Arc<Mutex<Vec<Box<dyn FnOnce() + Send>>>>;

struct Win32EventLoopProxy {
    hwnd: isize,
    queue: CallbackQueue,
}

unsafe impl Send for Win32EventLoopProxy {}
unsafe impl Sync for Win32EventLoopProxy {}

impl slint::platform::EventLoopProxy for Win32EventLoopProxy {
    fn quit_event_loop(&self) -> Result<(), slint::EventLoopError> {
        unsafe { PostQuitMessage(0); }
        Ok(())
    }
    fn invoke_from_event_loop(&self, event: Box<dyn FnOnce() + Send>) -> Result<(), slint::EventLoopError> {
        self.queue.lock().unwrap().push(event);
        unsafe { PostMessageW(self.hwnd as HWND, WM_SLINT_WAKE, 0, 0); }
        Ok(())
    }
}

// ── Class name (null-terminated UTF-16) ──────────────────────────────────────

const WND_CLASS: &[u16] = &[b'U' as u16, b'A' as u16, b'3' as u16, 0];
const CF_UNICODETEXT_VAL: u32 = 13;

// Thread-local storage for WndProc access
thread_local! {
    static WIN: RefCell<Option<Rc<Win32Window>>> = RefCell::new(None);
    // Called every event loop iteration from the main thread (for favicon progress etc.)
    static FRAME_CB: RefCell<Option<Box<dyn FnMut()>>> = RefCell::new(None);
    // Set by clear_frame_callback() — checked AFTER f() returns to avoid RefCell double-borrow
    static FRAME_CB_STOP: Cell<bool> = Cell::new(false);
}

pub fn set_frame_callback(f: impl FnMut() + 'static) {
    FRAME_CB_STOP.with(|c| c.set(false));
    FRAME_CB.with(|c| *c.borrow_mut() = Some(Box::new(f)));
}

pub fn clear_frame_callback() {
    // Safe to call from inside frame callback — only sets a Cell flag, no RefCell borrow
    FRAME_CB_STOP.with(|c| c.set(true));
}

// ── Public platform ───────────────────────────────────────────────────────────

pub struct Win32Platform {
    win: Rc<Win32Window>,
    queue: CallbackQueue,
}

impl Win32Platform {
    pub fn new(title: &str) -> Result<Self, String> {
        let inst = unsafe { GetModuleHandleW(std::ptr::null()) };
        register_class(inst)?;

        // Query system DPI so we render at native resolution (no OS bitmap stretch)
        let scale = unsafe {
            let hdc = GetDC(0);
            let dpi = GetDeviceCaps(hdc, 88 /* LOGPIXELSX */);
            ReleaseDC(0, hdc);
            (dpi as f32 / 96.0).max(1.0)
        };

        let lw = 960u32;
        let lh = 640u32;
        let pw = (lw as f32 * scale) as u32;
        let ph = (lh as f32 * scale) as u32;

        let slint_win = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        // ScaleFactorChanged must come first so set_size converts correctly
        slint_win.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: scale });
        slint_win.set_size(WindowSize::Physical(PhysicalSize::new(pw, ph)));

        let win = Rc::new(Win32Window {
            hwnd: Cell::new(0),
            slint_win,
            w: Cell::new(pw),
            h: Cell::new(ph),
            scale: Cell::new(scale),
            rgb_buf: RefCell::new(Vec::new()),
            bgr_buf: RefCell::new(Vec::new()),
        });

        let hwnd = create_hwnd(inst, title, pw as i32, ph as i32)?;
        win.hwnd.set(hwnd);

        WIN.with(|v| *v.borrow_mut() = Some(win.clone()));
        Ok(Win32Platform { win, queue: Arc::new(Mutex::new(Vec::new())) })
    }
}

impl Platform for Win32Platform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.win.slint_win.clone())
    }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn slint::platform::EventLoopProxy>> {
        Some(Box::new(Win32EventLoopProxy {
            hwnd: self.win.hwnd.get() as isize,
            queue: self.queue.clone(),
        }))
    }

    fn run_event_loop(&self) -> Result<(), PlatformError> {
        let hwnd = self.win.hwnd.get();
        unsafe {
            ShowWindow(hwnd, SW_SHOWDEFAULT);
            UpdateWindow(hwnd);
        }

        // Force initial repaint so the first frame is never missed
        self.win.slint_win.request_redraw();

        loop {
            // Drain invoke_from_event_loop callbacks from background threads
            let callbacks: Vec<_> = self.queue.lock().unwrap().drain(..).collect();
            for cb in callbacks { cb(); }

            // Per-frame callback (favicon progress, etc.)
            FRAME_CB.with(|c| { if let Some(f) = c.borrow_mut().as_mut() { f(); } });
            // Check stop flag AFTER f() returned — avoids RefCell double-borrow
            if FRAME_CB_STOP.with(|c| c.get()) {
                FRAME_CB_STOP.with(|c| c.set(false));
                FRAME_CB.with(|c| *c.borrow_mut() = None);
            }

            slint::platform::update_timers_and_animations();
            self.win.render(hwnd);

            let mut msg: MSG = unsafe { std::mem::zeroed() };
            while unsafe { PeekMessageW(&mut msg, 0, 0, 0, PM_REMOVE) } != 0 {
                if msg.message == WM_QUIT { return Ok(()); }
                unsafe {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }

            let ms = slint::platform::duration_until_next_timer_update()
                .map(|d| d.as_millis() as u32).unwrap_or(16).min(16);
            if ms > 0 { unsafe { Sleep(ms); } }
        }
    }

    fn set_clipboard_text(&self, text: &str, _: slint::platform::Clipboard) {
        clipboard_set(text);
    }

    fn clipboard_text(&self, _: slint::platform::Clipboard) -> Option<String> {
        clipboard_get()
    }
}

// ── Win32Window: wraps MinimalSoftwareWindow + HWND ──────────────────────────

struct Win32Window {
    hwnd: Cell<HWND>,
    slint_win: Rc<MinimalSoftwareWindow>,
    w: Cell<u32>,
    h: Cell<u32>,
    scale: Cell<f32>,
    // Pre-allocated render buffers reused across frames to avoid per-frame heap allocation.
    // At 150% DPI a 960×640 window becomes 1440×960 = ~1.4M pixels; allocating each frame
    // is expensive in debug builds and wastes GC pressure in release builds.
    rgb_buf: RefCell<Vec<slint::Rgb8Pixel>>,
    bgr_buf: RefCell<Vec<u8>>,
}

impl Win32Window {
    fn render(&self, hwnd: HWND) {
        self.slint_win.draw_if_needed(|renderer| {
            let w = self.w.get() as usize;
            let h = self.h.get() as usize;
            if w == 0 || h == 0 { return; }
            let needed = w * h;
            let mut rgb_buf = self.rgb_buf.borrow_mut();
            let mut bgr_buf = self.bgr_buf.borrow_mut();
            if rgb_buf.len() != needed {
                rgb_buf.resize(needed, slint::Rgb8Pixel::default());
                bgr_buf.resize(needed * 3, 0u8);
            }
            renderer.render(&mut rgb_buf, w);
            // In-place RGB→BGR swap into pre-allocated bgr_buf
            for (i, p) in rgb_buf.iter().enumerate() {
                let o = i * 3;
                bgr_buf[o]     = p.b;
                bgr_buf[o + 1] = p.g;
                bgr_buf[o + 2] = p.r;
            }
            blit_bgr(hwnd, &bgr_buf, w, h);
        });
    }

    fn mouse(&self, x: i16, y: i16, pressed: Option<bool>, btn: PointerEventButton) {
        let s = self.scale.get();
        let pos = LogicalPosition::new(x as f32 / s, y as f32 / s);
        match pressed {
            None    => self.slint_win.dispatch_event(WindowEvent::PointerMoved { position: pos }),
            Some(t) if t => self.slint_win.dispatch_event(WindowEvent::PointerPressed  { position: pos, button: btn }),
            _       => self.slint_win.dispatch_event(WindowEvent::PointerReleased { position: pos, button: btn }),
        }
    }

    fn scroll(&self, x: i32, y: i32, dy: f32) {
        let s = self.scale.get();
        self.slint_win.dispatch_event(WindowEvent::PointerScrolled {
            position: LogicalPosition::new(x as f32 / s, y as f32 / s),
            delta_x: 0.0,
            delta_y: dy,
        });
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn blit_bgr(hwnd: HWND, bgr: &[u8], w: usize, h: usize) {
    unsafe {
        let hdc = GetDC(hwnd);
        if hdc == 0 { return; }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 24,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0, biYPelsPerMeter: 0,
                biClrUsed: 0, biClrImportant: 0,
            },
            bmiColors: [RGBQUAD { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbReserved: 0 }],
        };
        StretchDIBits(
            hdc, 0, 0, w as i32, h as i32,
            0, 0, w as i32, h as i32,
            bgr.as_ptr() as *const _,
            &bmi, DIB_RGB_COLORS, SRCCOPY,
        );
        ReleaseDC(hwnd, hdc);
    }
}

// ── WndProc ───────────────────────────────────────────────────────────────────

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    WIN.with(|cell| {
        let borrow = cell.borrow();
        let win = match borrow.as_ref() { Some(w) => w, None => return DefWindowProcW(hwnd, msg, wp, lp) };

        match msg {
            WM_ERASEBKGND => 1, // prevent background repaint over our rendered content
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                BeginPaint(hwnd, &mut ps);
                win.render(hwnd);
                EndPaint(hwnd, &ps);
                0
            }
            WM_SIZE if wp != SIZE_MINIMIZED as usize => {
                let w = (lp & 0xFFFF) as u32;
                let h = ((lp >> 16) & 0xFFFF) as u32;
                if w > 0 && h > 0 {
                    win.w.set(w); win.h.set(h);
                    let s = win.scale.get();
                    win.slint_win.dispatch_event(WindowEvent::Resized {
                        size: slint::LogicalSize::new(w as f32 / s, h as f32 / s),
                    });
                }
                0
            }
            WM_MOUSEMOVE => {
                win.mouse((lp & 0xFFFF) as i16, ((lp >> 16) & 0xFFFF) as i16, None, PointerEventButton::Other);
                0
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => { SetCapture(hwnd); win.mouse((lp&0xFFFF) as i16, ((lp>>16)&0xFFFF) as i16, Some(true),  PointerEventButton::Left); 0 }
            WM_LBUTTONUP   => { ReleaseCapture(); win.mouse((lp&0xFFFF) as i16, ((lp>>16)&0xFFFF) as i16, Some(false), PointerEventButton::Left); 0 }
            WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => { win.mouse((lp&0xFFFF) as i16, ((lp>>16)&0xFFFF) as i16, Some(true),  PointerEventButton::Right); 0 }
            WM_RBUTTONUP   => { win.mouse((lp&0xFFFF) as i16, ((lp>>16)&0xFFFF) as i16, Some(false), PointerEventButton::Right); 0 }

            WM_MOUSEWHEEL => {
                let delta = ((wp >> 16) & 0xFFFF) as i16 as f32 / 120.0 * 15.0;
                let mut pt = POINT { x: (lp & 0xFFFF) as i16 as i32, y: ((lp >> 16) & 0xFFFF) as i16 as i32 };
                ScreenToClient(hwnd, &mut pt);
                win.scroll(pt.x, pt.y, delta);
                0
            }

            WM_KEYDOWN | WM_SYSKEYDOWN => {
                if let Some(k) = vk_to_key(wp) {
                    win.slint_win.dispatch_event(WindowEvent::KeyPressed { text: k });
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_KEYUP | WM_SYSKEYUP => {
                if let Some(k) = vk_to_key(wp) {
                    win.slint_win.dispatch_event(WindowEvent::KeyReleased { text: k });
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_CHAR => {
                if wp >= 32 && wp != 127 {
                    if let Some(c) = char::from_u32(wp as u32) {
                        let s: slint::SharedString = c.into();
                        win.slint_win.dispatch_event(WindowEvent::KeyPressed  { text: s.clone() });
                        win.slint_win.dispatch_event(WindowEvent::KeyReleased { text: s });
                    }
                }
                0
            }

            WM_SETFOCUS   => { win.slint_win.dispatch_event(WindowEvent::WindowActiveChanged(true));  0 }
            WM_KILLFOCUS  => { win.slint_win.dispatch_event(WindowEvent::WindowActiveChanged(false)); 0 }
            WM_CLOSE | WM_DESTROY => { PostQuitMessage(0); 0 }

            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    })
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn register_class(inst: isize) -> Result<(), String> {
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize:        std::mem::size_of::<WNDCLASSEXW>() as u32,
            style:         CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc:   Some(wnd_proc),
            cbClsExtra:    0,
            cbWndExtra:    0,
            hInstance:     inst,
            hIcon:         LoadIconW(inst, 1u16 as *const u16),  // icon resource ID 1
            hCursor:       LoadCursorW(0, IDC_ARROW),
            hbrBackground: (COLOR_WINDOW + 1) as HBRUSH,
            lpszMenuName:  std::ptr::null(),
            lpszClassName: WND_CLASS.as_ptr(),
            hIconSm:       0,
        };
        let atom = RegisterClassExW(&wc);
        if atom == 0 {
            let e = GetLastError();
            if e != ERROR_CLASS_ALREADY_EXISTS {
                return Err(format!("RegisterClassExW error {e}"));
            }
        }
        Ok(())
    }
}

fn create_hwnd(inst: isize, title: &str, cw: i32, ch: i32) -> Result<HWND, String> {
    let mut title_w: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let style = WS_OVERLAPPEDWINDOW;
        let mut rc = RECT { left: 0, top: 0, right: cw, bottom: ch };
        AdjustWindowRect(&mut rc, style, 0);
        let ww = rc.right - rc.left;
        let wh = rc.bottom - rc.top;
        let sx = (GetSystemMetrics(SM_CXSCREEN) - ww) / 2;
        let sy = (GetSystemMetrics(SM_CYSCREEN) - wh) / 2;
        let hwnd = CreateWindowExW(
            0, WND_CLASS.as_ptr(), title_w.as_mut_ptr(),
            style, sx, sy, ww, wh, 0, 0, inst, std::ptr::null(),
        );
        if hwnd == 0 { Err(format!("CreateWindowExW error {}", GetLastError())) }
        else { Ok(hwnd) }
    }
}

fn vk_to_key(vk: WPARAM) -> Option<slint::SharedString> {
    let k = match vk as u16 {
        VK_LEFT   => Key::LeftArrow,  VK_RIGHT  => Key::RightArrow,
        VK_UP     => Key::UpArrow,    VK_DOWN   => Key::DownArrow,
        VK_HOME   => Key::Home,       VK_END    => Key::End,
        VK_PRIOR  => Key::PageUp,     VK_NEXT   => Key::PageDown,
        VK_RETURN => Key::Return,     VK_ESCAPE => Key::Escape,
        VK_BACK   => Key::Backspace,  VK_DELETE => Key::Delete,
        VK_INSERT => Key::Insert,     VK_TAB    => Key::Tab,
        VK_F1  => Key::F1,  VK_F2  => Key::F2,  VK_F3  => Key::F3,
        VK_F4  => Key::F4,  VK_F5  => Key::F5,  VK_F6  => Key::F6,
        VK_F7  => Key::F7,  VK_F8  => Key::F8,  VK_F9  => Key::F9,
        VK_F10 => Key::F10, VK_F11 => Key::F11, VK_F12 => Key::F12,
        _ => return None,
    };
    Some(k.into())
}

// ── Clipboard ─────────────────────────────────────────────────────────────────

fn clipboard_set(text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(0) == 0 { return; }
        EmptyClipboard();
        let bytes = wide.len() * 2;
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes); // HGLOBAL = *mut c_void
        if !h.is_null() {
            let p = GlobalLock(h) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
                GlobalUnlock(h);
                SetClipboardData(CF_UNICODETEXT_VAL, h as HANDLE);
            }
        }
        CloseClipboard();
    }
}

fn clipboard_get() -> Option<String> {
    unsafe {
        if OpenClipboard(0) == 0 { return None; }
        let h = GetClipboardData(CF_UNICODETEXT_VAL); // HANDLE = isize
        if h == 0 { CloseClipboard(); return None; }
        let p = GlobalLock(h as HGLOBAL) as *const u16;
        if p.is_null() { CloseClipboard(); return None; }
        let mut n = 0usize;
        while *p.add(n) != 0 { n += 1; }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
        GlobalUnlock(h as HGLOBAL);
        CloseClipboard();
        Some(text)
    }
}
