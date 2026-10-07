#![windows_subsystem = "windows"]

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, BOOL, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, RECT,
    WPARAM,
};
use windows::Win32::Graphics::Gdi::{GetStockObject, BLACK_BRUSH, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Shell::{
    SHAppBarMessage, ABE_BOTTOM, ABM_ACTIVATE, ABM_GETTASKBARPOS, ABM_NEW, ABM_QUERYPOS,
    ABM_REMOVE, ABM_SETPOS, ABM_WINDOWPOSCHANGED, ABN_FULLSCREENAPP, ABN_POSCHANGED, APPBARDATA,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, IDC_ARROW, LoadCursorW,
    PostMessageW, PostQuitMessage, RegisterClassExW, RegisterWindowMessageW, SetForegroundWindow,
    SetWindowPos, TrackPopupMenu, TranslateMessage, HWND_BOTTOM, HWND_TOPMOST, MF_DISABLED,
    MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, TPM_RIGHTBUTTON, WINDOWPOS, WM_ACTIVATE, WM_COMMAND, WM_CONTEXTMENU,
    WM_CREATE, WM_DESTROY, WM_NULL, WM_WINDOWPOSCHANGED, WM_WINDOWPOSCHANGING, WNDCLASSEXW,
    WNDCLASS_STYLES, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};

/// Menu command id for the "Exit" entry in the right-click context menu.
const ID_EXIT: u32 = 1;
/// Short git commit sha this binary was built from, baked in by build.rs.
const GIT_SHA: &str = env!("SHADOWTASKBAR_GIT_SHA");

/// Height of the reserved bar, in pixels. Overridable via the first CLI arg.
static BAR_HEIGHT: AtomicI32 = AtomicI32::new(48);
/// Custom message id used by the shell to notify this appbar of changes.
static APPBAR_MSG_ID: AtomicU32 = AtomicU32::new(0);
/// Guards WM_WINDOWPOSCHANGING so our own SetWindowPos calls aren't clamped.
static IN_RESIZE: AtomicBool = AtomicBool::new(false);

fn main() {
    unsafe {
        // Ensure only one instance ever reserves the bar.
        let mutex = CreateMutexW(None, BOOL(1), w!("Global\\ShadowTaskbarRsSingleInstance"))
            .expect("CreateMutexW failed");
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(mutex);
            return;
        }

        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        // Default to the real taskbar's own reserved thickness (it returns physical
        // pixels, matching GetSystemMetrics under our per-monitor-DPI-aware process),
        // so the bar looks the same size regardless of display scaling or the
        // Windows 11 taskbar-size setting. Still overridable via the first CLI arg.
        let mut taskbar_abd = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            ..Default::default()
        };
        if SHAppBarMessage(ABM_GETTASKBARPOS, &mut taskbar_abd) != 0 {
            let taskbar_height = taskbar_abd.rc.bottom - taskbar_abd.rc.top;
            if taskbar_height > 0 {
                BAR_HEIGHT.store(taskbar_height, Ordering::SeqCst);
            }
        }
        if let Some(h) = std::env::args().nth(1).and_then(|s| s.parse::<i32>().ok()) {
            BAR_HEIGHT.store(h, Ordering::SeqCst);
        }

        let hinstance = GetModuleHandleW(None).expect("GetModuleHandleW failed");
        let class_name = w!("ShadowTaskbarRsClass");

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: WNDCLASS_STYLES(0),
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassExW(&wc);

        APPBAR_MSG_ID.store(
            RegisterWindowMessageW(w!("ShadowTaskbarRsCallback")),
            Ordering::SeqCst,
        );

        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let height = BAR_HEIGHT.load(Ordering::SeqCst);

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            class_name,
            w!("ShadowTaskbar"),
            WS_POPUP | WS_VISIBLE,
            0,
            screen_h - height,
            screen_w,
            height,
            None,
            None,
            hinstance,
            None,
        )
        .expect("CreateWindowExW failed");

        let _ = hwnd; // window is driven entirely via wndproc + the message loop below

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let _ = CloseHandle(mutex);
    }
}

/// Runs the ABM_QUERYPOS / ABM_SETPOS dance to reserve screen space along the bottom edge,
/// then applies the position/size the system actually granted us.
unsafe fn update_position(hwnd: HWND) {
    let screen_w = GetSystemMetrics(SM_CXSCREEN);
    let screen_h = GetSystemMetrics(SM_CYSCREEN);
    let height = BAR_HEIGHT.load(Ordering::SeqCst);

    let mut abd = APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        hWnd: hwnd,
        uCallbackMessage: APPBAR_MSG_ID.load(Ordering::SeqCst),
        uEdge: ABE_BOTTOM, // NOTE: if this doesn't typecheck, ABE_BOTTOM may be a wrapped enum - try `ABE_BOTTOM.0` or `ABE_BOTTOM.0 as u32`
        rc: RECT {
            left: 0,
            top: 0,
            right: screen_w,
            bottom: screen_h,
        },
        lParam: LPARAM(0),
    };

    SHAppBarMessage(ABM_QUERYPOS, &mut abd);
    abd.rc.top = abd.rc.bottom - height;
    SHAppBarMessage(ABM_SETPOS, &mut abd);

    IN_RESIZE.store(true, Ordering::SeqCst);
    let _ = SetWindowPos(
        hwnd,
        None,
        abd.rc.left,
        abd.rc.top,
        abd.rc.right - abd.rc.left,
        abd.rc.bottom - abd.rc.top,
        SWP_NOZORDER | SWP_NOACTIVATE,
    );
    IN_RESIZE.store(false, Ordering::SeqCst);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            let mut abd = APPBARDATA {
                cbSize: std::mem::size_of::<APPBARDATA>() as u32,
                hWnd: hwnd,
                uCallbackMessage: APPBAR_MSG_ID.load(Ordering::SeqCst),
                ..Default::default()
            };
            SHAppBarMessage(ABM_NEW, &mut abd);
            update_position(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            let mut abd = APPBARDATA {
                cbSize: std::mem::size_of::<APPBARDATA>() as u32,
                hWnd: hwnd,
                ..Default::default()
            };
            SHAppBarMessage(ABM_REMOVE, &mut abd);
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_ACTIVATE => {
            let mut abd = APPBARDATA {
                cbSize: std::mem::size_of::<APPBARDATA>() as u32,
                hWnd: hwnd,
                uCallbackMessage: APPBAR_MSG_ID.load(Ordering::SeqCst),
                ..Default::default()
            };
            SHAppBarMessage(ABM_ACTIVATE, &mut abd);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_WINDOWPOSCHANGED => {
            let mut abd = APPBARDATA {
                cbSize: std::mem::size_of::<APPBARDATA>() as u32,
                hWnd: hwnd,
                uCallbackMessage: APPBAR_MSG_ID.load(Ordering::SeqCst),
                ..Default::default()
            };
            SHAppBarMessage(ABM_WINDOWPOSCHANGED, &mut abd);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_CONTEXTMENU => {
            let x = (lparam.0 as i32) as i16 as i32;
            let y = ((lparam.0 as i32) >> 16) as i16 as i32;
            let (x, y) = if x == -1 && y == -1 {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                (pt.x, pt.y)
            } else {
                (x, y)
            };

            if let Ok(hmenu) = CreatePopupMenu() {
                let header = HSTRING::from(format!("ShadowTaskbar ({GIT_SHA})"));
                let _ = AppendMenuW(hmenu, MF_STRING | MF_DISABLED | MF_GRAYED, 0, &header);
                let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, None);
                let _ = AppendMenuW(hmenu, MF_STRING, ID_EXIT as usize, w!("Exit"));
                let _ = SetForegroundWindow(hwnd);
                let _ = TrackPopupMenu(hmenu, TPM_RIGHTBUTTON, x, y, 0, hwnd, None);
                let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
                let _ = DestroyMenu(hmenu);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            if (wparam.0 & 0xFFFF) as u32 == ID_EXIT {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_WINDOWPOSCHANGING => {
            if !IN_RESIZE.load(Ordering::SeqCst) {
                let wp = lparam.0 as *mut WINDOWPOS;
                if !wp.is_null() {
                    (*wp).flags |= SWP_NOMOVE | SWP_NOSIZE;
                }
            }
            LRESULT(0)
        }
        m if m == APPBAR_MSG_ID.load(Ordering::SeqCst) => {
            match wparam.0 as u32 {
                x if x == ABN_POSCHANGED => update_position(hwnd),
                x if x == ABN_FULLSCREENAPP => {
                    let target = if lparam.0 != 0 { HWND_BOTTOM } else { HWND_TOPMOST };
                    let _ = SetWindowPos(
                        hwnd,
                        target,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
                _ => {}
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
