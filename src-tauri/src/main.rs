// On Windows in release, we manage console visibility at runtime (FreeConsole in GUI mode)
// instead of using the compile-time `windows_subsystem = "windows"` attribute, which would
// send stdout to null and break --mcp stdio communication.

fn main() {
    let is_mcp = std::env::args().any(|a| a == "--mcp");

    if is_mcp {
        run_mcp();
    } else {
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
    fn has_mcp_flag(args: &[&str]) -> bool {
        args.iter().any(|a| *a == "--mcp")
    }

    #[test]
    fn detects_mcp_flag() {
        assert!(has_mcp_flag(&["curated-thoughts", "--mcp"]));
    }

    #[test]
    fn no_false_positive_without_flag() {
        assert!(!has_mcp_flag(&["curated-thoughts"]));
    }
}
