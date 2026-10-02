#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
fn ensure_cli_console() {
    use windows_sys::Win32::Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_OUTPUT_HANDLE,
    };
    const CONOUT: &[u16] = &[67, 79, 78, 79, 85, 84, 36, 0];
    unsafe {
        let cur = GetStdHandle(STD_OUTPUT_HANDLE);
        if cur != 0 && cur != INVALID_HANDLE_VALUE {
            return;
        }
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        let h = CreateFileW(
            CONOUT.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            0,
        );
        if h != INVALID_HANDLE_VALUE {
            SetStdHandle(STD_OUTPUT_HANDLE, h);
            SetStdHandle(STD_ERROR_HANDLE, h);
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_webkit_workaround() {
    // WebKitGTK 的 DMABUF 渲染器在 Wayland 下会触发 Gdk Error 71（protocol
    // error）直接崩溃，NVIDIA 驱动上必现。回退到旧渲染路径。
    let on_wayland = match std::env::var("WAYLAND_DISPLAY") {
        Ok(v) => !v.trim().is_empty(),
        Err(_) => std::env::var("XDG_SESSION_TYPE").map(|v| v.trim() == "wayland").unwrap_or(false),
    };
    if on_wayland && std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    linux_webkit_workaround();
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--cli") {
        let cli_args: Vec<String> = args.iter().skip(pos + 1).cloned().collect();
        #[cfg(windows)]
        ensure_cli_console();
        let (out, code) = zcode_switch_lib::cli::run(&cli_args);
        use std::io::Write;
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.write_all(b"\n");
        let _ = stdout.flush();
        std::process::exit(code);
    }
    zcode_switch_lib::run()
}
