// On Windows in release builds, we manage console visibility at runtime (FreeConsole in GUI
// mode) instead of using the compile-time `windows_subsystem = "windows"` attribute, which
// would send stdout to null and break --mcp stdio communication.
// Debug builds intentionally keep the console attached for easier debugging.

fn has_mcp_flag<Args>(args: Args) -> bool
where
    Args: IntoIterator,
    Args::Item: AsRef<std::ffi::OsStr>,
{
    args.into_iter().any(|a| a.as_ref() == std::ffi::OsStr::new("--mcp"))
}

fn main() {
    let is_mcp = has_mcp_flag(std::env::args_os());

    if is_mcp {
        run_mcp();
    } else {
        #[cfg(not(debug_assertions))]
        hide_console_on_windows();
        tauri_app_lib::run();
    }
}

fn run_mcp() {
    #[cfg(feature = "mcp-server")]
    {
        if let Err(e) = tauri_app_lib::mcp_server::run() {
            eprintln!("curated-thoughts [--mcp] fatal: {e}");
            std::process::exit(1);
        }
        std::process::exit(0);
    }

    #[cfg(not(feature = "mcp-server"))]
    {
        eprintln!("curated-thoughts: binary was not compiled with --features mcp-server");
        std::process::exit(1);
    }
}

#[cfg(target_os = "windows")]
fn hide_console_on_windows() {
    // Safety: FreeConsole is safe to call with no preconditions.
    // It detaches the process from its console window so no terminal
    // flashes when launching the GUI from Explorer/Start menu.
    unsafe {
        windows_sys::Win32::System::Console::FreeConsole();
    }
}

#[cfg(not(target_os = "windows"))]
fn hide_console_on_windows() {}

#[cfg(test)]
mod dispatch_tests {
    fn has_mcp_flag(args: &[std::ffi::OsString]) -> bool {
        args.iter().any(|a| a == std::ffi::OsStr::new("--mcp"))
    }

    #[test]
    fn detects_mcp_flag() {
        let args = ["curated-thoughts", "--mcp"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        assert!(has_mcp_flag(&args));
    }

    #[test]
    fn no_false_positive_without_flag() {
        assert!(!has_mcp_flag(&[std::ffi::OsString::from("curated-thoughts")]));
    }
}
