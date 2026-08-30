// On Windows in release builds, we manage console visibility at runtime (FreeConsole in GUI
// mode) instead of using the compile-time `windows_subsystem = "windows"` attribute, which
// would send stdout to null and break --mcp stdio communication.
// Debug builds intentionally keep the console attached for easier debugging.

fn has_mcp_flag<Args>(args: Args) -> bool
where
    Args: IntoIterator,
    Args::Item: AsRef<std::ffi::OsStr>,
{
    args.into_iter()
        .any(|a| a.as_ref() == std::ffi::OsStr::new("--mcp"))
}

fn main() {
    let is_mcp = has_mcp_flag(std::env::args_os());

    if is_mcp {
        run_mcp();
    } else if let Err(code) = run_cli_subcommand() {
        std::process::exit(code);
    } else {
        #[cfg(not(debug_assertions))]
        hide_console_on_windows();
        tauri_app_lib::run();
    }
}

/// Returns `Ok(())` when a CLI subcommand was dispatched (caller should skip GUI launch).
/// Returns `Err(exit_code)` when the process should terminate with that exit code.
fn run_cli_subcommand() -> Result<(), i32> {
    let mut args = std::env::args_os().skip(1); // skip binary name
    let Some(first) = args.next() else {
        return Ok(()); // no subcommand
    };

    match first.to_str() {
        Some("--onboard") => {
            let mut vault_path: Option<String> = None;
            let mut force = false;
            let mut iter = args;
            while let Some(arg) = iter.next() {
                match arg.to_str() {
                    Some("--force") => force = true,
                    Some("--vault") => {
                        vault_path = iter.next().and_then(|v| v.to_str().map(String::from));
                    }
                    _ => {}
                }
            }
            match tauri_app_lib::onboard::run_onboard(tauri_app_lib::onboard::OnboardOptions {
                vault_path,
                force,
            }) {
                Ok(_) => Err(0),
                Err(e) => {
                    eprintln!("onboard error: {e}");
                    Err(1)
                }
            }
        }
        _ => Ok(()), // not a CLI subcommand — fall through to GUI
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

#[cfg(all(target_os = "windows", not(debug_assertions)))]
fn hide_console_on_windows() {
    // Safety: these Windows console APIs are called with valid arguments:
    // - GetConsoleWindow and FreeConsole have no preconditions for this use.
    // - GetConsoleProcessList is given a valid writable buffer and its length.
    // Detach only when a console is attached and this process is its sole client,
    // preserving terminal output for users who launch the app from an existing
    // command prompt while still hiding a transient console created for the GUI app.
    unsafe {
        if windows_sys::Win32::System::Console::GetConsoleWindow() == std::ptr::null_mut() {
            return;
        }

        let mut process_list = [0u32; 2];
        let process_count = windows_sys::Win32::System::Console::GetConsoleProcessList(
            process_list.as_mut_ptr(),
            process_list.len() as u32,
        );

        if process_count == 1 {
            windows_sys::Win32::System::Console::FreeConsole();
        }
    }
}

#[cfg(all(not(target_os = "windows"), not(debug_assertions)))]
fn hide_console_on_windows() {}

#[cfg(test)]
mod dispatch_tests {
    use super::has_mcp_flag;

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
        assert!(!has_mcp_flag(&[std::ffi::OsString::from(
            "curated-thoughts"
        )]));
    }
}
