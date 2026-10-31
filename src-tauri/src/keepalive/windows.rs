use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::Read,
    net::{SocketAddr, TcpStream},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
use tungstenite::{protocol::WebSocketConfig, Message};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Instance {
    pub pid: u32,
    pub created_ms: i64,
    pub executable: String,
    #[serde(default)]
    pub ports: Vec<u16>,
}

fn process(request: Value) -> Result<Value, String> {
    use std::os::windows::process::CommandExt;
    let script: Vec<u8> = include_str!("process.ps1")
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(script);
    let mut child = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &encoded,
        ])
        .env("ZSW_KEEPALIVE_REQUEST", request.to_string())
        .creation_flags(0x0800_0000)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Could not run the Windows process helper".to_owned())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Windows process helper timed out; operation must be reconciled".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        // Do not forward PowerShell's error record, command line, or environment.
        return Err("ZCode instance verification or lifecycle operation failed".into());
    }
    serde_json::from_slice(&output.stdout).map_err(|_| "Invalid process helper response".into())
}

pub fn inspect(executable: &str, ports: bool) -> Result<Option<Instance>, String> {
    serde_json::from_value(process(
        json!({"action":"inspect","executable":executable,"ports":ports}),
    )?)
    .map_err(|_| "Invalid process identity".into())
}

pub fn stop(instance: &Instance) -> Result<(), String> {
    process(
        json!({"action":"stop","executable":instance.executable,"pid":instance.pid,"created_ms":instance.created_ms}),
    )?;
    Ok(())
}

pub fn start(executable: &str) -> Result<Instance, String> {
    serde_json::from_value(process(json!({"action":"start","executable":executable}))?)
        .map_err(|_| "Invalid launched process identity".into())
}

pub fn same_instance(a: &Instance, b: &Instance) -> bool {
    a.pid == b.pid
        && a.created_ms == b.created_ms
        && a.executable.eq_ignore_ascii_case(&b.executable)
}

pub struct Renderer {
    pub instance: Instance,
    port: u16,
    path: String,
}

impl Renderer {
    pub fn discover(instance: &Instance) -> Result<Self, String> {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(2))
            .build();
        for port in instance.ports.iter().take(8) {
            let response = match agent
                .get(&format!("http://127.0.0.1:{port}/json/list"))
                .call()
            {
                Ok(r) => r,
                Err(_) => continue,
            };
            let mut raw = String::new();
            if response
                .into_reader()
                .take(256 * 1024)
                .read_to_string(&mut raw)
                .is_err()
            {
                continue;
            }
            let Ok(targets) = serde_json::from_str::<Vec<Value>>(&raw) else {
                continue;
            };
            for target in targets {
                if target["type"] != "page" {
                    continue;
                }
                let Some(url) = target["url"].as_str() else {
                    continue;
                };
                if !is_zcode_renderer(url, &instance.executable) {
                    continue;
                }
                let Some(ws) = target["webSocketDebuggerUrl"].as_str() else {
                    continue;
                };
                let prefix = format!("ws://127.0.0.1:{port}/devtools/page/");
                let Some(id) = ws.strip_prefix(&prefix) else {
                    continue;
                };
                if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                    continue;
                }
                return Ok(Self {
                    instance: instance.clone(),
                    port: *port,
                    path: format!("/devtools/page/{id}"),
                });
            }
        }
        Err("Waiting for the ZCode desktop control connection".into())
    }

    pub fn evaluate(&self, expression: &str) -> Result<Value, String> {
        let current =
            inspect(&self.instance.executable, true)?.ok_or("ZCode is no longer running")?;
        if !same_instance(&current, &self.instance) || !current.ports.contains(&self.port) {
            return Err("ZCode control connection ownership changed".into());
        }
        let address: SocketAddr = ([127, 0, 0, 1], self.port).into();
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))
            .map_err(|_| "ZCode control connection unavailable")?;
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .map_err(|e| e.to_string())?;
        let url = format!("ws://127.0.0.1:{}{}", self.port, self.path);
        let config = WebSocketConfig {
            max_message_size: Some(2 * 1024 * 1024),
            max_frame_size: Some(2 * 1024 * 1024),
            ..Default::default()
        };
        let (mut socket, _) =
            tungstenite::client::client_with_config(url.as_str(), stream, Some(config))
                .map_err(|_| "ZCode control handshake failed")?;
        let request = json!({"id":1,"method":"Runtime.evaluate","params":{"expression":expression,"returnByValue":true,"awaitPromise":true,"timeout":15000}});
        socket
            .send(Message::Text(request.to_string()))
            .map_err(|_| "Could not send ZCode control request")?;
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            let msg = socket
                .read()
                .map_err(|_| "ZCode control response was interrupted")?;
            let Message::Text(text) = msg else { continue };
            let value: Value =
                serde_json::from_str(&text).map_err(|_| "Invalid ZCode control response")?;
            if value["id"] != 1 {
                continue;
            }
            let _ = socket.close(None);
            if value.get("error").is_some() || value["result"].get("exceptionDetails").is_some() {
                return Err("ZCode does not support the required keepalive operation".into());
            }
            return Ok(value["result"]["result"]["value"].clone());
        }
        Err("ZCode control request timed out".into())
    }
}

fn is_zcode_renderer(url: &str, executable: &str) -> bool {
    let Some(parent) = Path::new(executable).parent() else {
        return false;
    };
    let expected = format!(
        "file:///{}/resources/app.asar/out/renderer/index.html",
        parent
            .to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
    );
    // Decode only URL escapes, never accept another scheme, host, or renderer path.
    let decoded =
        percent_encoding::percent_decode_str(url.split('?').next().unwrap_or(url)).decode_utf8();
    decoded
        .map(|s| s.eq_ignore_ascii_case(&expected))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_selected_installation_renderer_is_accepted() {
        assert!(is_zcode_renderer("file:///C:/Program%20Files/ZCode/resources/app.asar/out/renderer/index.html?restoreSession=true", r"C:\Program Files\ZCode\ZCode.exe"));
        for url in [
            "https://evil.test/index.html",
            "file:///D:/other/resources/app.asar/out/renderer/index.html",
            "file:///D:/ZCode/resources/app.asar/out/renderer/index.html.evil",
        ] {
            assert!(!is_zcode_renderer(url, r"D:\ZCode\ZCode.exe"));
        }
    }
    #[test]
    fn windows_helper_encodes_absent_instance_as_json_null() {
        let nonexistent = std::env::temp_dir().join(format!(
            "zswitch-absent-{}\\ZCode.exe",
            uuid::Uuid::new_v4()
        ));
        assert!(inspect(&nonexistent.to_string_lossy(), false)
            .unwrap()
            .is_none());
    }
    #[test]
    #[ignore = "requires an explicitly selected isolated ZCode fixture"]
    fn isolated_installed_renderer_uses_the_real_windows_and_cdp_bridge() {
        let executable =
            std::env::var("ZSW_KEEPALIVE_TEST_EXE").expect("set the isolated fixture executable");
        let instance = inspect(&executable, true)
            .unwrap()
            .expect("fixture running");
        let renderer = Renderer::discover(&instance).unwrap();
        renderer.evaluate(include_str!("renderer.js")).unwrap();
        let result = renderer
            .evaluate("globalThis.__zswitchKeepalive.probe()")
            .unwrap();
        assert_eq!(result["supported"], true);
        assert_eq!(result["clientMode"], "desktop-continuous");
    }
    #[test]
    #[ignore = "restarts an explicitly isolated installation on a hidden test desktop"]
    fn isolated_process_lifecycle_restarts_only_the_selected_installation() {
        let executable = std::env::var("ZSW_KEEPALIVE_TEST_EXE").expect("isolated executable");
        let root = std::env::var("ZSW_KEEPALIVE_TEST_ROOT").expect("isolated root");
        let home = std::env::var("ZCODE_DESKTOP_HOME_DIR").expect("isolated home");
        assert!(Path::new(&executable).starts_with(&root));
        assert!(Path::new(&home).starts_with(&root));
        assert_eq!(std::env::var("ZCODE_DATA_BASE_DIR").unwrap(), home);
        assert_eq!(
            std::env::var("ZCODE_DESKTOP_APPLICATION_NAME").unwrap(),
            "ZCode Keepalive Probe"
        );
        let before = inspect(&executable, false)
            .unwrap()
            .expect("fixture running");
        stop(&before).unwrap();
        assert!(inspect(&executable, false).unwrap().is_none());
        let after = start(&executable).unwrap();
        assert!(!same_instance(&before, &after));
        let deadline = std::time::Instant::now() + Duration::from_secs(45);
        let ready = loop {
            let current = inspect(&executable, true).unwrap().unwrap();
            assert!(same_instance(&after, &current));
            if Renderer::discover(&current).is_ok() {
                break current;
            }
            assert!(std::time::Instant::now() < deadline, "renderer not ready");
            std::thread::sleep(Duration::from_secs(1));
        };
        std::fs::write(
            Path::new(&root).join("lifecycle-proof.json"),
            serde_json::to_vec_pretty(&json!({"before":before,"after":ready,"result":"passed"}))
                .unwrap(),
        )
        .unwrap();
    }
}
