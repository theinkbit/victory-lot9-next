//! A debugging overlay over the title's window: where a frame's time goes,
//! what is dropped or refused and by which program, and the card's and the
//! PC's memory. A window of ours owned by the title's window (it stays above
//! that window only, not above others), that takes no clicks and no focus.
//!
//! - F10 (while the title's window has the focus) shows and hides it.
//! - P switches catch-up on and off: a frame waits for every pipeline it
//!   draws with instead of dropping its draws, and each present waits until
//!   no pipeline is being made (at most a minute). The title all but stops
//!   while the renderer catches up; the frames it draws are whole.
//! - `VICTORY_DEBUG_UI=0` turns it off; `VICTORY_SHADER_NAMES=<file>` names
//!   programs by hash (`hash name` lines, as `victory translate --cache`
//!   writes `shader-names.txt` in some trees).
//!
//! The numbers are those of the frame-rate line in the log, refreshed when
//! it is (every few seconds).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// What the frame-rate line measured, per frame.
#[derive(Clone, Default)]
pub(crate) struct Frame {
    pub fps: f64,
    pub ms: f64,
    pub preparing: f64,
    pub copies: f64,
    pub textures: f64,
    pub gpu: f64,
    pub back: f64,
    pub heap_passes: f64,
    pub draws: f64,
    pub dropped: f64,
    pub indirect: f64,
    pub making: u64,
    pub textures_waiting: u64,
}

static FRAME: Mutex<Option<Frame>> = Mutex::new(None);
/// The programs dropped most, per frame: (what, how many a frame).
static DROPS: Mutex<Vec<(String, f64)>> = Mutex::new(Vec::new());
static SHOWN: AtomicBool = AtomicBool::new(true);
static CATCH_UP: AtomicBool = AtomicBool::new(false);

/// Whether catch-up (P) is on.
pub(crate) fn catch_up() -> bool {
    CATCH_UP.load(Ordering::Relaxed)
}

/// At a present, with catch-up on: waits (at most a minute) until no
/// pipeline is being made.
pub(crate) fn hold_for_catch_up() {
    if !catch_up() {
        return;
    }
    let started = std::time::Instant::now();
    while catch_up()
        && crate::pipeline::making() > 0
        && started.elapsed() < std::time::Duration::from_secs(60)
    {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
static WINDOW: AtomicUsize = AtomicUsize::new(0);

fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_DEBUG_UI").is_none_or(|v| v != "0"))
}

/// The frame-rate line's numbers.
pub(crate) fn frame(f: Frame) {
    *FRAME.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
}

/// The programs dropped most since the last frame-rate line.
pub(crate) fn drops(list: Vec<(String, f64)>) {
    *DROPS.lock().unwrap_or_else(|e| e.into_inner()) = list;
}

/// Shows the overlay over `hwnd` (the window the title presents to).
pub(crate) fn attach(hwnd: usize) {
    if hwnd == 0 || !on() {
        return;
    }
    if WINDOW.swap(hwnd, Ordering::Relaxed) == hwnd {
        return;
    }
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let _ = std::thread::Builder::new().name("victory-debug-ui".into()).spawn(win::run);
    });
}

/// `hash name` lines from `VICTORY_SHADER_NAMES`.
fn names() -> &'static std::collections::HashMap<String, String> {
    static NAMES: std::sync::OnceLock<std::collections::HashMap<String, String>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        std::env::var_os("VICTORY_SHADER_NAMES")
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|text| {
                text.lines()
                    .filter_map(|l| l.split_once(' '))
                    .map(|(h, n)| (h.to_owned(), n.trim().to_owned()))
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// `text` with each 16-hex-digit program hash followed by its name, shortened.
fn named(text: &str) -> String {
    let names = names();
    let mut out = String::new();
    for word in text.split(' ') {
        let key = word.trim_matches(|c: char| !c.is_ascii_hexdigit());
        out.push_str(word);
        if key.len() == 16
            && let Some(name) = names.get(key)
        {
            // `group:CS_Name_1234abcd_Wrapped` -> `CS_Name`
            let short = name.rsplit(':').next().unwrap_or(name);
            let short = short.trim_end_matches("_Wrapped");
            let short = short.rsplit_once('_').filter(|(_, h)| h.len() == 8).map_or(short, |(s, _)| s);
            out.push_str(&format!(" ({short})"));
        }
        out.push(' ');
    }
    out.trim_end().to_owned()
}

/// The lines of each panel, with a colour (COLORREF, BGR) each.
fn panels() -> (Vec<(u32, String)>, Vec<(u32, String)>) {
    const TEXT: u32 = 0x00e0_e0e0;
    const DIM: u32 = 0x00a0_a0a0;
    const BLUE: u32 = 0x00f0_c8a0;
    const YELLOW: u32 = 0x0000_d8f0;
    const RED: u32 = 0x006060ff;
    const GREEN: u32 = 0x0060_d060;
    let mut left = Vec::new();
    match FRAME.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        Some(f) => {
            let colour = if f.fps >= 20.0 { GREEN } else if f.fps >= 5.0 { YELLOW } else { RED };
            left.push((colour, format!("{:.1} fps  {:.0} ms/frame  {:.0} draws", f.fps, f.ms, f.draws)));
            left.push((TEXT, format!("preparing {:.0} ms: copies {:.0}, textures {:.0}", f.preparing, f.copies, f.textures)));
            left.push((TEXT, format!("GPU {:.0} ms   copy-back {:.0} ms   heap passes {:.0} ms", f.gpu, f.back, f.heap_passes)));
            let drop_colour = if f.dropped > 0.5 { YELLOW } else { DIM };
            left.push((drop_colour, format!("dropped {:.0}/frame, {:.0} indirect", f.dropped, f.indirect)));
            if f.making > 0 {
                left.push((YELLOW, format!("pipelines being made: {}", f.making)));
            }
            if f.textures_waiting > 0 {
                left.push((YELLOW, format!("textures waiting: {}", f.textures_waiting)));
            }
        }
        None => left.push((DIM, "waiting for the first frame-rate line...".into())),
    }
    if catch_up() {
        left.push((0x0030_90ff, "CATCH-UP on (P): frames wait for every pipeline".into()));
    }
    if let Some([budget, used, _, shared]) = crate::host::host().and_then(|h| h.video_memory()) {
        let mut line = format!("VRAM {:.0} / {:.0} MB", used as f64 / 1048576.0, budget as f64 / 1048576.0);
        if shared > 64 << 20 {
            line.push_str(&format!(", {:.0} MB in PC memory", shared as f64 / 1048576.0));
        }
        left.push((BLUE, line));
    }
    if let Some((total, avail)) = ram() {
        let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
        left.push((BLUE, format!("RAM {:.1} / {:.0} GB", gb(total - avail), gb(total))));
    }
    let mut right = Vec::new();
    let drops = DROPS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if !drops.is_empty() {
        right.push((YELLOW, "Dropped per frame (no host pipeline):".into()));
        for (why, n) in drops.iter().take(8) {
            let mut text = named(why);
            if text.chars().count() > 62 {
                text = text.chars().take(61).collect::<String>() + "~";
            }
            right.push((TEXT, format!("{n:>6.1}  {text}")));
        }
    }
    (left, right)
}

fn ram() -> Option<(u64, u64)> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        // SAFETY: a plain structure of integers.
        ..unsafe { std::mem::zeroed() }
    };
    // SAFETY: a structure with its length set.
    (unsafe { GlobalMemoryStatusEx(&mut m) } != 0).then_some((m.ullTotalPhys, m.ullAvailPhys))
}

mod win {
    use super::{CATCH_UP, Ordering, SHOWN, WINDOW};
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_F10};
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::w;

    /// Transparent where painted in this colour.
    const KEY: u32 = 0x00ff_00ff;
    const HEIGHT: i32 = 250;
    const TIMER_PAINT: usize = 1;
    const TIMER_KEY: usize = 2;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    pub(super) fn run() {
        // SAFETY: plain window creation and a message loop on this thread.
        unsafe {
            let instance = GetModuleHandleW(None).unwrap_or_default();
            let class = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance.into(),
                lpszClassName: w!("VictoryDebugUi"),
                ..Default::default()
            };
            RegisterClassW(&class);
            let owner = HWND(WINDOW.load(Ordering::Relaxed) as *mut _);
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                w!("VictoryDebugUi"),
                w!("VIctory debug"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                Some(owner),
                None,
                Some(instance.into()),
                None,
            ) else {
                tracing::warn!("the debug overlay window could not be made");
                return;
            };
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(KEY), 230, LWA_COLORKEY | LWA_ALPHA);
            SetTimer(Some(hwnd), TIMER_PAINT, 500, None);
            SetTimer(Some(hwnd), TIMER_KEY, 50, None);
            tracing::info!("debug overlay on (F10 shows and hides it)");
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    /// Whether the title's window (or one of ours) has the keyboard.
    unsafe fn focused(game: HWND) -> bool {
        // SAFETY: plain queries.
        unsafe {
            let fg = GetForegroundWindow();
            if fg == game {
                return true;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(fg, Some(&mut pid));
            pid == std::process::id()
        }
    }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        // SAFETY: the window procedure of our own class.
        unsafe {
            let game = HWND(WINDOW.load(Ordering::Relaxed) as *mut _);
            match msg {
                WM_TIMER if wp.0 == TIMER_KEY => {
                    static WAS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                    let down = focused(game) && GetAsyncKeyState(i32::from(VK_F10.0)) < 0;
                    if down && !WAS.swap(true, Ordering::Relaxed) {
                        let shown = !SHOWN.fetch_xor(true, Ordering::Relaxed);
                        let _ = ShowWindow(hwnd, if shown { SW_SHOWNOACTIVATE } else { SW_HIDE });
                    } else if !down {
                        WAS.store(false, Ordering::Relaxed);
                    }
                    // P: catch-up on and off.
                    static WAS_P: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                    let p = focused(game) && GetAsyncKeyState(i32::from(b'P')) < 0;
                    if p && !WAS_P.swap(true, Ordering::Relaxed) {
                        let on = !CATCH_UP.fetch_xor(true, Ordering::Relaxed);
                        tracing::info!(on, "catch-up switched (P)");
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    } else if !p {
                        WAS_P.store(false, Ordering::Relaxed);
                    }
                    LRESULT(0)
                }
                WM_TIMER => {
                    if !SHOWN.load(Ordering::Relaxed)
                        || !IsWindow(Some(game)).as_bool()
                        || IsIconic(game).as_bool()
                        || !IsWindowVisible(game).as_bool()
                    {
                        let _ = ShowWindow(hwnd, SW_HIDE);
                        return LRESULT(0);
                    }
                    let mut r = RECT::default();
                    let _ = GetClientRect(game, &mut r);
                    let mut at = POINT { x: 0, y: 0 };
                    let _ = ClientToScreen(game, &mut at);
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        at.x,
                        at.y,
                        (r.right - r.left).max(1),
                        HEIGHT,
                        SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW,
                    );
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    LRESULT(0)
                }
                WM_PAINT => {
                    paint(hwnd);
                    LRESULT(0)
                }
                WM_ERASEBKGND => LRESULT(1),
                WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
                WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
                _ => DefWindowProcW(hwnd, msg, wp, lp),
            }
        }
    }

    unsafe fn paint(hwnd: HWND) {
        // SAFETY: painting our own window with objects made and freed here.
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut r = RECT::default();
            let _ = GetClientRect(hwnd, &mut r);
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            let mem = CreateCompatibleDC(Some(hdc));
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            let old_bmp = SelectObject(mem, bmp.into());
            let key = CreateSolidBrush(COLORREF(KEY));
            FillRect(mem, &r, key);
            let font = CreateFontW(
                18, 0, 0, 0, 500, 0, 0, 0,
                DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, NONANTIALIASED_QUALITY,
                (FF_MODERN.0 | FIXED_PITCH.0) as u32,
                w!("Consolas"),
            );
            let old_font = SelectObject(mem, font.into());
            SetBkMode(mem, TRANSPARENT);
            let (left, right) = super::panels();
            let panel = CreateSolidBrush(COLORREF(0x0020_2020));
            let line = 21;
            let width = |lines: &[(u32, String)]| {
                lines.iter().map(|l| l.1.chars().count()).max().unwrap_or(0) as i32 * 10 + 20
            };
            let draw = |x: i32, lines: &[(u32, String)]| {
                let b = RECT { left: x, top: 8, right: x + width(lines), bottom: 8 + lines.len() as i32 * line + 10 };
                FillRect(mem, &b, panel);
                for (i, (colour, text)) in lines.iter().enumerate() {
                    SetTextColor(mem, COLORREF(*colour));
                    let _ = TextOutW(mem, x + 10, 13 + i as i32 * line, &wide(text));
                }
            };
            if !left.is_empty() {
                draw(8, &left);
            }
            if !right.is_empty() {
                draw((w - width(&right) - 8).max(8), &right);
            }
            let _ = BitBlt(hdc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
            SelectObject(mem, old_font);
            SelectObject(mem, old_bmp);
            let _ = DeleteObject(font.into());
            let _ = DeleteObject(bmp.into());
            let _ = DeleteObject(key.into());
            let _ = DeleteObject(panel.into());
            let _ = DeleteDC(mem);
            let _ = EndPaint(hwnd, &ps);
        }
    }
}
