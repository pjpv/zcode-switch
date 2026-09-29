
use crate::quota;
use crate::zcrypto;
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const BILLING_PREVIEW_URL: &str = "https://zcode.z.ai/api/v1/zcode-plan/billing/preview";
pub const BILLING_CLAIM_URL: &str = "https://zcode.z.ai/api/v1/zcode-plan/billing/claim";
pub const CLIENT_CONFIGS_URL: &str = "https://zcode.z.ai/api/v1/client/configs";

const CLAIM_TIMEOUT_SECS: u64 = 25;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ClaimGrant {
    pub name: String,
    pub units: f64,
    pub period: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ClaimPlan {
    pub plan_id: String,
    pub name: String,
    pub description: String,
    pub priority: i64,
    pub grants: Vec<String>,
    #[serde(default)]
    pub grant_items: Vec<ClaimGrant>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptchaConfig {
    pub enabled: bool,
    pub region: String,
    pub prefix: String,
    pub scene_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimOutcome {
    pub account_id: String,
    pub account_name: String,
    pub plan_name: String,
    pub starts_at: Option<i64>,
    pub ends_at: Option<i64>,
    pub server_time: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ClaimError {
    pub code: i64,
    pub message: String,
    pub next_at: Option<i64>,
}

impl From<String> for ClaimError {
    fn from(message: String) -> Self {
        ClaimError {
            code: -1,
            message,
            next_at: None,
        }
    }
}

fn claim_error(code: i64, body: &Value) -> ClaimError {
    let next_at = if code == 1005 {
        body.pointer("/data/plan/ends_at")
            .and_then(|x| x.as_i64())
            .map(|s| s * 1000)
    } else {
        None
    };
    ClaimError {
        code,
        message: failure_message(code, body),
        next_at,
    }
}

pub fn failure_payload(
    account_id: &str,
    account_name: &str,
    plan_name: &str,
    err: &ClaimError,
) -> Value {
    serde_json::json!({
        "ok": false,
        "accountId": account_id,
        "accountName": account_name,
        "planName": plan_name,
        "code": err.code,
        "nextAt": err.next_at,
        "message": err.message,
    })
}

fn claim_token(creds: &Value, config: Option<&Value>, secret: &str) -> Result<String, String> {
    let jwt = creds
        .get("zcodejwttoken")
        .and_then(|v| v.as_str())
        .and_then(|v| decrypt_credential(v, secret));
    if let Some(t) = jwt.filter(|t| t.trim().len() > 20) {
        return Ok(t);
    }
    if let Some(t) = quota::zai_billing_token(creds, config, secret) {
        return Ok(t);
    }
    Err(crate::i18n::tr("err.claim.no_jwt"))
}

fn decrypt_credential(v: &str, secret: &str) -> Option<String> {
    if zcrypto::is_encrypted(v) {
        zcrypto::decrypt_with_secret(v, secret).ok()
    } else {
        Some(v.to_string())
    }
}

fn agent() -> ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(CLAIM_TIMEOUT_SECS))
        .build()).clone()
}

pub const EVENT_REPORT_URL: &str = "https://zcode.z.ai/api/v1/event/report";
pub const ACTIVATION_EVENTS: [&str; 2] = ["app_launch", "app_daily_active"];
const SCREEN_RESOLUTION: &str = "2560x1440";
const ACTIVATION_TIMEOUT_SECS: u64 = 10;

pub(crate) fn telemetry_user_id(home: &Path, creds: &Value) -> Option<String> {
    let secret = zcrypto::default_secret(home);
    let provider = creds
        .get("oauth:active_provider")
        .and_then(|v| v.as_str())
        .and_then(|v| decrypt_credential(v, &secret))
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "zai".to_string());
    let raw = creds
        .get(format!("oauth:{provider}:user_info"))?
        .as_str()?;
    let plain = decrypt_credential(raw, &secret)?;
    let info: Value = serde_json::from_str(&plain).ok()?;
    let pick = |k: &str| {
        info.get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    pick("id").or_else(|| pick("user_id")).map(String::from)
}

fn activation_event_body(element: &str, event_id: &str, user_id: &str, mid: &str) -> Value {
    serde_json::json!({
        "event_id": event_id,
        "client_timezone": quota::client_timezone(),
        "client_language": quota::ZCODE_LANG,
        "element_name": element,
        "event_region": "app",
        "event_type": "view",
        "event_text": "",
        "event_extra_detail": {},
        "user_id": user_id,
        "screen_resolution": SCREEN_RESOLUTION,
        "app_version": quota::zcode_app_version(),
        "device_os_category": device_os_category(),
        "device_os_version": quota::os_version().unwrap_or_default(),
        "device_mid": mid,
        "mac_id": "",
        "marketing_params": "{}",
    })
}

fn device_os_category() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "macos",
        _ => "linux",
    }
}

pub fn report_activation_events(user_id: &str, device_mid: &str) -> Result<(), String> {
    let agent = agent();
    for element in ACTIVATION_EVENTS {
        let body = activation_event_body(
            element,
            &uuid::Uuid::new_v4().to_string(),
            user_id,
            device_mid,
        );
        let resp = agent
            .post(EVENT_REPORT_URL)
            .timeout(Duration::from_secs(ACTIVATION_TIMEOUT_SECS))
            .set("Content-Type", "application/json")
            .send_json(body)
            .map_err(|e| {
                crate::i18n::trf("err.claim.activate_req", &[("e", http_err("", e).trim())])
            })?
            .into_string()
            .map_err(|e| crate::i18n::trf("err.claim.activate_req", &[("e", &e.to_string())]))?;
        let v: Value = serde_json::from_str(&resp).unwrap_or(Value::String(resp));
        let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        if code != 0 {
            return Err(failure_message(code, &v));
        }
    }
    Ok(())
}

pub fn preview_plans(
    home: &Path,
    creds: &Value,
    config: Option<&Value>,
    device_mid: Option<String>,
) -> Result<Vec<ClaimPlan>, String> {
    let secret = zcrypto::default_secret(home);
    let token = claim_token(creds, config, &secret)?;
    let url = format!(
        "{BILLING_PREVIEW_URL}?app_version={}&platform={}",
        quota::zcode_app_version(),
        quota::client_platform()
    );
    preview_once(&url, &token, device_mid)
}

fn http_err(prefix: &str, e: ureq::Error) -> String {
    if let ureq::Error::Status(code, resp) = e {
        let body = resp.into_string().unwrap_or_default();
        let msg = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| ["msg", "message", "error"]
                .iter()
                .find_map(|k| v.get(k).and_then(|x| x.as_str()).map(String::from)))
            .unwrap_or_default();
        return format!("{prefix} HTTP {code}: {msg}");
    }
    format!("{prefix} {e}")
}

fn preview_once(url: &str, token: &str, mid: Option<String>) -> Result<Vec<ClaimPlan>, String> {
    let mut req = agent().get(url);
    for (k, v) in quota::zai_billing_headers_with_mid(token, mid) {
        req = req.set(&k, &v);
    }
    let resp = req
        .call()
        .map_err(|e| http_err(&crate::i18n::tr("err.claim.preview_req"), e))?
        .into_string()
        .map_err(|e| crate::i18n::trf("err.http.read", &[("e", &e.to_string())]))?;
    let v: Value = serde_json::from_str(&resp).unwrap_or(Value::String(resp));
    let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        return Err(failure_message(code, &v));
    }
    let plans = v
        .pointer("/data/plans")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out: Vec<ClaimPlan> = plans.iter().filter_map(parse_plan).collect();
    out.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.plan_id.cmp(&b.plan_id)));
    Ok(out)
}

pub fn submit_claim(
    home: &Path,
    creds: &Value,
    config: Option<&Value>,
    plan_id: &str,
    captcha_param: &str,
    captcha_region: Option<&str>,
    device_mid: Option<String>,
) -> Result<Value, ClaimError> {
    if captcha_param.trim().is_empty() {
        return Err(crate::i18n::tr("err.claim.no_captcha").into());
    }
    let secret = zcrypto::default_secret(home);
    let token = claim_token(creds, config, &secret)?;
    let mut req = agent().post(BILLING_CLAIM_URL);
    for (k, v) in quota::zai_billing_headers_with_mid(&token, device_mid) {
        req = req.set(&k, &v);
    }
    req = req.set("X-Aliyun-Captcha-Verify-Param", captcha_param.trim());
    if let Some(r) = captcha_region.filter(|r| !r.trim().is_empty()) {
        req = req.set("X-Aliyun-Captcha-Verify-Region", r.trim());
    }
    let resp = req
        .send_json(serde_json::json!({ "plan_id": plan_id }))
        .map_err(|e| ClaimError {
            code: -1,
            message: http_err(&crate::i18n::tr("err.claim.claim_req"), e),
            next_at: None,
        })?
        .into_string()
        .map_err(|e| ClaimError {
            code: -1,
            message: crate::i18n::trf("err.http.read", &[("e", &e.to_string())]),
            next_at: None,
        })?;
    let v: Value = serde_json::from_str(&resp).unwrap_or(Value::String(resp));
    let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        return Err(claim_error(code, &v));
    }
    Ok(v)
}

#[derive(Debug)]
struct ConfigError {
    message: String,
    transient: bool,
}

impl ConfigError {
    fn permanent(message: String) -> Self { Self { message, transient: false } }
}

#[derive(Default)]
struct ConfigCache {
    value: Option<(Instant, CaptchaConfig)>,
}

impl ConfigCache {
    fn get(&mut self, mut fetch: impl FnMut() -> Result<CaptchaConfig, ConfigError>, mut pause: impl FnMut()) -> Result<CaptchaConfig, String> {
        if let Some((at, cfg)) = &self.value {
            if at.elapsed() < Duration::from_secs(300) { return Ok(cfg.clone()); }
        }
        self.value = None;
        let cfg = match fetch() {
            Err(e) if e.transient => { pause(); fetch().map_err(|e| e.message)? }
            result => result.map_err(|e| e.message)?,
        };
        // Only successful, usable configuration is shared between accounts.
        if cfg.enabled && !cfg.scene_id.trim().is_empty() {
            self.value = Some((Instant::now(), cfg.clone()));
        }
        Ok(cfg)
    }
}

pub fn fetch_captcha_config() -> Result<CaptchaConfig, String> {
    static CACHE: Mutex<ConfigCache> = Mutex::new(ConfigCache { value: None });
    let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    cache.get(|| fetch_captcha_config_at(&agent(), CLIENT_CONFIGS_URL), || {
        std::thread::sleep(Duration::from_millis(300));
    })
}

fn fetch_captcha_config_at(agent: &ureq::Agent, url: &str) -> Result<CaptchaConfig, ConfigError> {
    let mut req = agent.get(url).timeout(Duration::from_secs(12));
    for (k, v) in quota::zai_billing_headers("") {
        req = req.set(&k, &v);
    }
    let resp = req
        .call()
        .map_err(|e| ConfigError {
            transient: matches!(&e, ureq::Error::Transport(t) if matches!(t.kind(),
                ureq::ErrorKind::ConnectionFailed | ureq::ErrorKind::Dns | ureq::ErrorKind::Io)),
            message: http_err(&crate::i18n::tr("err.claim.config_unavailable"), e),
        })?
        .into_string()
        .map_err(|e| ConfigError { transient: true, message: crate::i18n::trf("err.http.read", &[("e", &e.to_string())]) })?;
    let v: Value = serde_json::from_str(&resp)
        .map_err(|_| ConfigError::permanent(crate::i18n::tr("err.claim.config_unavailable")))?;
    if v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1) != 0 {
        return Err(ConfigError::permanent(crate::i18n::tr("err.claim.config_unavailable")));
    }
    let c = v.pointer("/data/configs/captcha").cloned().unwrap_or(Value::Null);
    let s = |k: &str| c.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
    Ok(CaptchaConfig {
        enabled: c.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false),
        region: s("region"),
        prefix: s("prefix"),
        scene_id: s("sceneId"),
    })
}

fn str_field<'a>(e: &'a Value, snake: &str, camel: &str) -> Option<&'a str> {
    e.get(snake)
        .and_then(|v| v.as_str())
        .or_else(|| e.get(camel).and_then(|v| v.as_str()))
}

fn num_field(e: &Value, snake: &str, camel: &str) -> Option<f64> {
    e.get(snake)
        .and_then(|v| v.as_f64())
        .or_else(|| e.get(camel).and_then(|v| v.as_f64()))
}

fn parse_plan(p: &Value) -> Option<ClaimPlan> {
    let plan_id = str_field(p, "plan_id", "planId")?.trim().to_string();
    if plan_id.is_empty() {
        return None;
    }
    let grant_items: Vec<ClaimGrant> = p
        .get("entitlements")
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|e| {
                    str_field(e, "meter", "meter") == Some("model_usage")
                        && str_field(e, "unit_type", "unitType") == Some("token")
                        && str_field(e, "show_name", "showName")
                            .map(|s| !s.trim().is_empty())
                            .unwrap_or(false)
                })
                .map(|e| ClaimGrant {
                    name: str_field(e, "show_name", "showName").unwrap_or("").to_string(),
                    units: num_field(e, "grant_units", "grantUnits").unwrap_or(0.0),
                    period: str_field(e, "period", "period").unwrap_or("one_time").to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    let grants = grant_items
        .iter()
        .map(|g| {
            let period_cn = match g.period.as_str() {
                "daily" => "每日",
                "weekly" => "每周",
                "monthly" => "每月",
                _ => "一次性",
            };
            format!("{} · {} Token（{period_cn}）", g.name, fmt_units(g.units))
        })
        .collect();
    Some(ClaimPlan {
        name: p.get("name").and_then(|s| s.as_str()).unwrap_or("").trim().to_string(),
        description: p
            .get("description")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        priority: p.get("priority").and_then(|x| x.as_i64()).unwrap_or(0),
        plan_id,
        grants,
        grant_items,
    })
}

fn fmt_units(n: f64) -> String {
    let trim = |x: f64| {
        let r = (x * 10.0).round() / 10.0;
        if (r - r.trunc()).abs() < f64::EPSILON {
            format!("{}", r.trunc() as i64)
        } else {
            format!("{r:.1}")
        }
    };
    if n >= 1e8 {
        format!("{}亿", trim(n / 1e8))
    } else if n >= 1e4 {
        format!("{}万", trim(n / 1e4))
    } else {
        format!("{}", n.round() as i64)
    }
}

pub fn failure_message(code: i64, body: &Value) -> String {
    let server_msg = ["msg", "message"]
        .iter()
        .find_map(|k| body.get(k).and_then(|x| x.as_str()).map(String::from))
        .unwrap_or_default();
    let base = crate::i18n::tr(match code {
        1001 => "claim.fail.1001",
        1002 => "claim.fail.1002",
        1003 => "claim.fail.1003",
        1004 => "claim.fail.1004",
        1005 => "claim.fail.1005",
        3001 => "claim.fail.3001",
        3007 => "claim.fail.3007",
        401 => "claim.fail.401",
        _ => "claim.fail.generic",
    });
    if server_msg.is_empty() {
        base
    } else {
        crate::i18n::trf("claim.fail.with_server", &[("base", &base), ("server_msg", &server_msg)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn config() -> CaptchaConfig {
        CaptchaConfig { enabled: true, scene_id: "scene".into(), region: "sgp".into(), prefix: "prefix".into() }
    }

    #[test]
    fn transient_config_error_retries_once_then_caches_success() {
        let mut cache = ConfigCache::default();
        let (mut calls, mut pauses) = (0, 0);
        let result = cache.get(|| {
            calls += 1;
            if calls == 1 { Err(ConfigError { transient: true, message: "timeout".into() }) }
            else { Ok(config()) }
        }, || pauses += 1).unwrap();
        assert_eq!(result.scene_id, "scene");
        assert_eq!((calls, pauses), (2, 1));
        cache.get(|| panic!("fresh cache must avoid HTTP"), || panic!()).unwrap();
        cache.value.as_mut().unwrap().0 = Instant::now() - Duration::from_secs(301);
        cache.get(|| { calls += 1; Ok(config()) }, || panic!()).unwrap();
        assert_eq!(calls, 3);
    }

    #[test]
    fn failure_is_not_cached_and_retries_are_bounded() {
        let mut cache = ConfigCache::default();
        let mut calls = 0;
        assert!(cache.get(|| {
            calls += 1;
            Err(ConfigError { transient: true, message: "offline".into() })
        }, || {}).is_err());
        assert_eq!(calls, 2);
        assert!(cache.value.is_none());
        cache.get(|| Ok(config()), || panic!()).unwrap();
    }

    #[test]
    fn permanent_error_and_disabled_config_are_not_retried_or_cached() {
        let mut cache = ConfigCache::default();
        assert!(cache.get(|| Err(ConfigError::permanent("HTTP 401".into())), || panic!()).is_err());
        let mut disabled = config(); disabled.enabled = false;
        assert!(!cache.get(|| Ok(disabled.clone()), || panic!()).unwrap().enabled);
        assert!(cache.value.is_none());
        let mut missing_scene = config(); missing_scene.scene_id.clear();
        cache.get(|| Ok(missing_scene.clone()), || panic!()).unwrap();
        assert!(cache.value.is_none());
    }

    fn http_config(status: u16, body: &str) -> Result<CaptchaConfig, ConfigError> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/configs", listener.local_addr().unwrap());
        let body = body.to_string();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut request = [0; 8192];
            socket.read(&mut request).unwrap();
            write!(socket, "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        });
        let agent = ureq::AgentBuilder::new().try_proxy_from_env(false).build();
        let result = fetch_captcha_config_at(&agent, &url);
        server.join().unwrap();
        result
    }

    #[test]
    fn real_http_config_path_parses_success_and_classifies_failures() {
        let cfg = http_config(200, r#"{"code":0,"data":{"configs":{"captcha":{"enabled":true,"region":"sgp","prefix":"p","sceneId":"s"}}}}"#).unwrap();
        assert_eq!(cfg.scene_id, "s");
        for (status, body) in [(401, r#"{"msg":"unauthorized"}"#), (429, "{}"), (200, "not json"), (200, r#"{"code":3007}"#)] {
            let err = http_config(status, body).unwrap_err();
            assert!(!err.transient, "status {status} must not be retried");
        }
    }
}
