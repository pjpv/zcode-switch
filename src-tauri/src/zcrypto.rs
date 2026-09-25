
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

pub const PREFIX: &str = "enc:v1:";

pub fn node_platform_for<'a>(os: &'a str) -> &'a str {
    match os {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
}

fn node_os() -> &'static str {
    node_platform_for(std::env::consts::OS)
}

fn pick_username(username: Option<&str>, user: Option<&str>, logname: Option<&str>) -> String {
    username
        .or(user)
        .or(logname)
        .unwrap_or("unknown")
        .to_string()
}

#[cfg(not(windows))]
fn passwd_username() -> Option<String> {
    let out = std::process::Command::new("id")
        .arg("-un")
        .output()
        .ok()?;
    let n = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!n.is_empty()).then_some(n)
}

fn compose_fallback_secret(platform: &str, home: &str, username: &str) -> String {
    format!("zcode-credential-fallback:{}:{}:{}", platform, home, username)
}

pub fn default_secret(home: &Path) -> String {
    if let Ok(s) = std::env::var("ZCODE_CREDENTIAL_SECRET") {
        return s;
    }
    #[cfg(windows)]
    let primary = std::env::var("USERNAME").ok();
    #[cfg(not(windows))]
    let primary = passwd_username().or_else(|| std::env::var("USER").ok());
    let username = pick_username(
        primary.as_deref(),
        std::env::var("USER").ok().as_deref(),
        std::env::var("LOGNAME").ok().as_deref(),
    );
    compose_fallback_secret(node_os(), &home.display().to_string(), &username)
}

fn derive_key(secret: &str) -> [u8; 32] {
    let d = Sha256::digest(secret.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&d);
    out
}

pub fn is_encrypted(v: &str) -> bool {
    v.starts_with(PREFIX)
}

pub fn decrypt_with_secret(value: &str, secret: &str) -> Result<String, String> {
    let body = value.strip_prefix(PREFIX).ok_or("不是 enc:v1 格式")?;
    let parts: Vec<&str> = body.split('.').collect();
    if parts.len() != 3 {
        return Err("enc:v1 格式不正确".into());
    }
    let nonce_b = URL_SAFE_NO_PAD.decode(parts[0]).map_err(|e| format!("nonce 解码失败：{e}"))?;
    let tag_b = URL_SAFE_NO_PAD.decode(parts[1]).map_err(|e| format!("tag 解码失败：{e}"))?;
    let ct_b = URL_SAFE_NO_PAD.decode(parts[2]).map_err(|e| format!("密文解码失败：{e}"))?;
    if nonce_b.len() != 12 {
        return Err("nonce 长度异常".into());
    }
    let key = derive_key(secret);
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| format!("密钥初始化失败：{e}"))?;
    let mut buf = ct_b.clone();
    buf.extend_from_slice(&tag_b);
    let pt = cipher
        .decrypt(Nonce::from_slice(&nonce_b), buf.as_slice())
        .map_err(|_| "解密失败（密钥不匹配或数据损坏）".to_string())?;
    Ok(String::from_utf8_lossy(&pt).to_string())
}

pub fn decrypt_json_opt(value: Option<&str>, secret: &str) -> Option<Value> {
    let v = value?;
    let plain = if is_encrypted(v) { decrypt_with_secret(v, secret).ok()? } else { v.to_string() };
    serde_json::from_str(&plain).ok()
}

pub fn decode_jwt(jwt: &str) -> Option<Value> {
    let mut parts = jwt.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    if payload.is_empty() {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn jwt_exp_ms(jwt: &str) -> Option<i64> {
    let p = decode_jwt(jwt)?;
    let exp = p.get("exp")?;
    exp.as_i64().or_else(|| exp.as_f64().map(|f| f as i64))
        .map(|s| s.saturating_mul(1000))
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Identity {
    pub provider: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub user_id: Option<String>,
}

impl Identity {
    pub fn label(&self) -> Option<String> {
        self.display_name
            .clone()
            .or_else(|| self.username.clone())
            .or_else(|| self.email.clone())
    }
}

pub fn identity_with_secret(creds: &Value, secret: &str) -> Identity {
    let mut id = Identity { provider: "bigmodel".into(), ..Default::default() };
    let map = match creds.as_object() {
        Some(m) => m,
        None => return id,
    };
    if let Some(ap) = map.get("oauth:active_provider").and_then(|v| v.as_str()) {
        let plain = if is_encrypted(ap) { decrypt_with_secret(ap, secret).ok() } else { Some(ap.to_string()) };
        if let Some(p) = plain.filter(|p| !p.is_empty()) {
            id.provider = p;
        }
    }
    let ui_key = format!("oauth:{}:user_info", id.provider);
    let ui = map
        .get(ui_key.as_str())
        .and_then(|v| v.as_str())
        .and_then(|v| decrypt_json_opt(Some(v), secret));
    if let Some(ui) = ui {
        id.username = ui.get("username").and_then(|v| v.as_str()).map(String::from);
        id.display_name = ui.get("displayName").and_then(|v| v.as_str()).map(String::from);
        id.email = ui
            .get("email")
            .and_then(|v| v.as_str())
            .or_else(|| ui.get("rawProfile").and_then(|r| r.get("email")).and_then(|v| v.as_str()))
            .map(String::from);
        if let Some(uid) = ui.get("id").filter(|v| !v.is_null()) {
            id.user_id = uid.as_str().map(String::from).or_else(|| serde_json::to_string(uid).ok());
        }
    }
    if id.user_id.is_none() {
        let at_key = format!("oauth:{}:access_token", id.provider);
        if let Some(at) = map.get(at_key.as_str()).and_then(|v| v.as_str()) {
            let plain = if is_encrypted(at) { decrypt_with_secret(at, secret).ok() } else { Some(at.to_string()) };
            if let Some(jwt) = plain.as_deref().and_then(decode_jwt) {
                id.user_id = jwt
                    .get("user_id")
                    .or_else(|| jwt.get("sub"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
            }
        }
    }
    id
}

pub fn account_identity(creds: &Value, home: &Path) -> Identity {
    identity_with_secret(creds, &default_secret(home))
}
