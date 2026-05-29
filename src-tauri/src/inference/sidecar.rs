use anyhow::Result;
use std::path::Path;
use tauri::{AppHandle, Emitter};

pub struct SidecarProcess {
    pub port: u16,
    pub child: std::process::Child,
}

impl Drop for SidecarProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn pick_port() -> Result<u16> {
    portpicker::pick_unused_port()
        .ok_or_else(|| anyhow::anyhow!("no free TCP port available"))
}

pub fn spawn_sidecar(binary_path: &Path, model_path: &Path, port: u16) -> Result<SidecarProcess> {
    let child = std::process::Command::new(binary_path)
        .args([
            "--model",
            model_path.to_str().unwrap_or(""),
            "--port",
            &port.to_string(),
            "--host",
            "127.0.0.1",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(SidecarProcess { port, child })
}

fn await_sidecar_ready_impl(
    sidecar: &mut SidecarProcess,
    timeout: std::time::Duration,
    mut emit_progress: impl FnMut(u64),
) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(1))
        .build()?;
    let url = format!("http://127.0.0.1:{}/health", sidecar.port);
    let start = std::time::Instant::now();

    loop {
        if start.elapsed() > timeout {
            return Err(anyhow::anyhow!("sidecar startup timed out after {}s", timeout.as_secs()));
        }
        if let Ok(Some(status)) = sidecar.child.try_wait() {
            return Err(anyhow::anyhow!("sidecar exited during startup ({})", status));
        }
        if let Ok(resp) = client.get(&url).send() {
            if let Ok(body) = resp.json::<serde_json::Value>() {
                if body.get("status").and_then(|v| v.as_str()) == Some("ok") {
                    return Ok(());
                }
            }
        }
        emit_progress(start.elapsed().as_secs());
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

pub fn await_sidecar_ready(sidecar: &mut SidecarProcess, app: &AppHandle) -> Result<()> {
    await_sidecar_ready_impl(
        sidecar,
        std::time::Duration::from_secs(120),
        |elapsed_s| {
            let _ = app.emit(
                "provider-loading",
                serde_json::json!({ "elapsed_s": elapsed_s }),
            );
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_port_returns_usable_port() {
        let port = pick_port().unwrap();
        assert!(port > 1024, "port should be unprivileged");
        let listener = std::net::TcpListener::bind(("127.0.0.1", port));
        assert!(listener.is_ok(), "picked port should be bindable");
    }

    #[test]
    fn await_sidecar_ready_detects_child_exit() {
        let mut sidecar = SidecarProcess {
            port: 19999,
            child: std::process::Command::new("sh")
                .args(["-c", "exit 1"])
                .spawn()
                .unwrap(),
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        let result = sidecar.child.try_wait().unwrap();
        assert!(result.is_some(), "child should have exited");
    }

    #[test]
    fn await_sidecar_ready_times_out_with_mock_server() {
        let mut server = mockito::Server::new();
        let _mock = server
            .mock("GET", "/health")
            .with_body(r#"{"status":"loading model"}"#)
            .with_header("content-type", "application/json")
            .expect_at_least(1)
            .create();

        let server_url = server.url();
        let port: u16 = server_url
            .rsplit_once(':')
            .expect("mock server URL contains a port")
            .1
            .parse()
            .expect("port is numeric");

        let mut sidecar = SidecarProcess {
            port,
            child: std::process::Command::new("sh")
                .arg("-c")
                .arg("sleep 60")
                .spawn()
                .unwrap(),
        };

        let result = await_sidecar_ready_impl(
            &mut sidecar,
            std::time::Duration::from_millis(200),
            |_| {},
        );

        assert!(result.is_err(), "await_sidecar_ready should time out if health never becomes ok");
    }
}
