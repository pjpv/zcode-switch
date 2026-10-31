
use crate::quota;
use crate::zcrypto;
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimRefreshResult {
    pub plans: Vec<ClaimPlan>,
    pub activated: bool,
    pub activation_error: Option<String>,
}

pub fn refresh_account(paths: &crate::store::Paths, id: &str) -> Result<ClaimRefreshResult, String> {
    let acc = crate::store::load_account(paths, id)?;
    let mid = crate::store::account_claim_mid(paths, &acc)?;
    let secret = zcrypto::default_secret(&paths.home);
    let token = claim_token(&acc.credentials, acc.config.as_ref(), &secret)?;
    let (activated, activation_error) = match telemetry_user_id(&paths.home, &acc.credentials) {
        Some(uid) => match report_activation_events(&uid, &mid, &token) {
            Ok(()) => (true, None),
            Err(e) => (false, Some(e)),
        },
        None => (false, None),
    };
    let plans = preview_plans(&paths.home, &acc.credentials, acc.config.as_ref(), Some(mid))?;
    Ok(ClaimRefreshResult { plans, activated, activation_error })
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

/// curl 转发场景下复刻原 ureq http_err 的错误文本：从响应体 JSON 里提取 msg。
fn status_err(prefix: &str, status: u16, body: &str) -> String {
    let msg = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| ["msg", "message", "error"]
            .iter()
            .find_map(|k| v.get(k).and_then(|x| x.as_str()).map(String::from)))
        .unwrap_or_default();
    if status == 0 {
        format!("{prefix} network error")
    } else {
        format!("{prefix} HTTP {status}: {msg}")
    }
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

pub fn report_activation_events(user_id: &str, device_mid: &str, token: &str) -> Result<(), String> {
    report_activation_events_at(EVENT_REPORT_URL, user_id, device_mid, token)
}

fn report_activation_events_at(
    url: &str,
    user_id: &str,
    device_mid: &str,
    token: &str,
) -> Result<(), String> {
    for element in ACTIVATION_EVENTS {
        let body = activation_event_body(
            element,
            &uuid::Uuid::new_v4().to_string(),
            user_id,
            device_mid,
        );
        let body_text = serde_json::to_string(&body)
            .map_err(|e| crate::i18n::trf("err.claim.activate_req", &[("e", &e.to_string())]))?;
        let mut headers = quota::zai_billing_headers_with_mid(token, Some(device_mid.to_string()));
        headers.push(("Content-Type".into(), "application/json".into()));
        let (status, resp) =
            quota::zcode_post_json(url, &headers, &body_text, ACTIVATION_TIMEOUT_SECS)
                .map_err(|e| crate::i18n::trf("err.claim.activate_req", &[("e", &e)]))?;
        if status == 0 || status >= 400 {
            return Err(crate::i18n::trf(
                "err.claim.activate_req",
                &[("e", &status_err("", status, &resp).trim())],
            ));
        }
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
    // 认证视角失败（网络/WAF）时同样尝试匿名回退，避免活动一变就整体探测不到。
    let (auth_err, mut out) = match preview_once(&url, &token, device_mid.clone()) {
        Ok(v) => (None, v),
        Err(e) => (Some(e), vec![]),
    };
    if out.is_empty() {
        // 2026-10-30 起 zcode.z.ai 的 billing/preview 对携带有效 JWT 的请求
        // 不再返回可领计划（一律空 data），导致界面永远不显示「可领」。
        // 回退：用同一设备的匿名视角（不带 Authorization）获取当前活动计划，
        // 再用 billing/balance 查出该账号已持有的 plan_id 并排除——
        // 剩下的才是该账号真正可领的。匿名视角按设备 MID 判定资格，
        // 设备领取过一次后同样返回空，因此无需额外去重。
        if let Some(anon) = preview_anonymous(&url, device_mid.clone()) {
            // owned 查询失败时降级为空集合：宁可多显示让服务器裁决，
            // 不能因为一次辅助查询失败就漏掉新活动。
            let owned = owned_plan_ids(&token, device_mid).unwrap_or_default();
            out = anon
                .into_iter()
                .filter(|p| !owned.contains(&p.plan_id))
                .collect();
        } else if let Some(e) = auth_err {
            return Err(e);
        }
    }
    Ok(out)
}

/// 匿名视角的活动预览：与 preview_once 相同的请求，但不带 Authorization。
fn preview_anonymous(url: &str, mid: Option<String>) -> Option<Vec<ClaimPlan>> {
    let headers: Vec<(String, String)> = quota::zai_billing_headers_with_mid("", mid)
        .into_iter()
        .filter(|(k, _)| !k.eq_ignore_ascii_case("Authorization"))
        .collect();
    let v = quota::zcode_fetch_json_with_headers(url, &headers, false).ok()?;
    if v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1) != 0 {
        return None;
    }
    let plans = v
        .pointer("/data/plans")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out: Vec<ClaimPlan> = plans.iter().filter_map(parse_plan).collect();
    out.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.plan_id.cmp(&b.plan_id)));
    Some(out)
}

/// 该账号在 billing/balance 里已持有的 plan_id 列表（None = 查询失败，不回退）。
fn owned_plan_ids(token: &str, mid: Option<String>) -> Option<Vec<String>> {
    let url = format!(
        "{}?app_version={}",
        quota::BILLING_BALANCE_URL,
        quota::zcode_app_version()
    );
    let headers = quota::zai_billing_headers_with_mid(token, mid);
    let v = quota::zcode_fetch_json_with_headers(&url, &headers, false).ok()?;
    if v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1) != 0 {
        return None;
    }
    Some(
        v.pointer("/data/plans")
            .and_then(|p| p.as_array())
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|p| p.get("plan_id").and_then(|x| x.as_str()).map(String::from))
            .collect(),
    )
}

fn preview_once(url: &str, token: &str, mid: Option<String>) -> Result<Vec<ClaimPlan>, String> {
    let headers = quota::zai_billing_headers_with_mid(token, mid);
    let v = quota::zcode_fetch_json_with_headers(url, &headers, false)
        .map_err(|e| format!("{} {e}", crate::i18n::tr("err.claim.preview_req")))?;
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
    let mut headers = quota::zai_billing_headers_with_mid(&token, device_mid);
    headers.push(("X-Aliyun-Captcha-Verify-Param".into(), captcha_param.trim().to_string()));
    if let Some(r) = captcha_region.map(str::trim).filter(|r| !r.is_empty()) {
        headers.push(("X-Aliyun-Captcha-Verify-Region".into(), r.to_string()));
    }
    headers.push(("Content-Type".into(), "application/json".into()));
    let body = serde_json::json!({ "plan_id": plan_id }).to_string();
    let (status, resp) =
        quota::zcode_post_json(BILLING_CLAIM_URL, &headers, &body, CLAIM_TIMEOUT_SECS)
            .map_err(|e| ClaimError {
                code: -1,
                message: format!("{} {e}", crate::i18n::tr("err.claim.claim_req")),
                next_at: None,
            })?;
    if status == 0 || status >= 400 {
        return Err(ClaimError {
            code: -1,
            message: status_err(&crate::i18n::tr("err.claim.claim_req"), status, &resp),
            next_at: None,
        });
    }
    let v: Value = serde_json::from_str(&resp).unwrap_or(Value::String(resp));
    let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        return Err(claim_error(code, &v));
    }
    Ok(v)
}

pub fn fetch_captcha_config() -> Result<CaptchaConfig, String> {
    let headers = quota::zai_billing_headers("");
    let v = quota::zcode_fetch_json_with_headers(CLIENT_CONFIGS_URL, &headers, false)
        .map_err(|e| crate::i18n::trf("err.claim.config_req", &[("e", &e)]))?;
    if v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1) != 0 {
        return Err(crate::i18n::tr("err.claim.config_unavailable"));
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
mod activation_tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    fn mock_reports(
        responses: Vec<&'static str>,
    ) -> (String, thread::JoinHandle<Vec<(HashMap<String, String>, Value)>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/event/report", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let server = thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            let mut requests = Vec::new();
            for response in responses {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(std::time::Instant::now() < deadline, "missing activation request");
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(e) => panic!("mock accept failed: {e}"),
                    }
                };
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert_eq!(line.trim(), "POST /event/report HTTP/1.1");
                let mut headers = HashMap::new();
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    let (name, value) = line.split_once(':').unwrap();
                    headers.insert(name.to_ascii_lowercase(), value.trim().to_string());
                }
                let length = headers["content-length"].parse::<usize>().unwrap();
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                requests.push((headers, serde_json::from_slice(&body).unwrap()));
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.len(), response
                ).unwrap();
            }
            requests
        });
        (url, server)
    }

    #[test]
    fn activation_reports_authenticate_both_events_with_existing_device_context() {
        let (url, server) = mock_reports(vec![r#"{"code":0}"#, r#"{"code":0}"#]);
        report_activation_events_at(&url, "existing-user", "existing-mid", "test-session-token").unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        let version = quota::zcode_app_version();
        for ((headers, body), element) in requests.iter().zip(ACTIVATION_EVENTS) {
            assert_eq!(headers["authorization"], "Bearer test-session-token");
            assert_eq!(headers["content-type"], "application/json");
            assert_eq!(headers["x-device-mid"], "existing-mid");
            assert_eq!(headers["user-agent"], format!("ZCode/{version}"));
            assert_eq!(headers["x-zcode-app-version"], version);
            assert_eq!(headers["x-platform"], quota::client_platform());
            assert_eq!(headers["x-title"], "Z Code@electron");
            assert_eq!(headers["http-referer"], "https://zcode.z.ai");
            assert_eq!(headers["x-release-channel"], "stable");
            assert_eq!(headers["x-client-language"], quota::ZCODE_LANG);
            assert_eq!(headers["x-client-timezone"], quota::client_timezone());
            assert_eq!(headers["x-os-category"], std::env::consts::OS);
            if let Some(os_version) = quota::os_version() {
                assert_eq!(headers["x-os-version"], os_version);
            }
            uuid::Uuid::parse_str(&headers["x-request-id"]).unwrap();
            let event_id = body["event_id"].as_str().unwrap();
            uuid::Uuid::parse_str(event_id).unwrap();
            assert_eq!(*body, activation_event_body(element, event_id, "existing-user", "existing-mid"));
        }
        assert_ne!(requests[0].1["event_id"], requests[1].1["event_id"]);
    }

    #[test]
    fn activation_reports_preserve_server_failure() {
        let (url, server) = mock_reports(vec![r#"{"code":3007,"msg":"not eligible"}"#]);
        let error = report_activation_events_at(&url, "existing-user", "existing-mid", "test-token")
            .unwrap_err();
        assert!(error.contains("not eligible"));
        assert_eq!(server.join().unwrap().len(), 1);
    }
}
