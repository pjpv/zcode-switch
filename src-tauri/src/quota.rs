
use crate::zcrypto;
use serde_json::Value;
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

#[cfg(windows)]
fn no_window(prog: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new(prog);
    c.creation_flags(0x0800_0000);
    c
}
#[cfg(not(windows))]
fn no_window(prog: &str) -> std::process::Command {
    std::process::Command::new(prog)
}

pub const QUOTA_LIMIT_URL: &str = "https://open.bigmodel.cn/api/monitor/usage/quota/limit";
pub const SUBSCRIPTION_URL: &str = "https://open.bigmodel.cn/api/biz/subscription/list";
pub const BILLING_BALANCE_URL: &str = "https://zcode.z.ai/api/v1/zcode-plan/billing/balance";
pub const CLIENT_APP_VERSION: &str = "3.11.2";

pub(crate) fn client_platform() -> String {
    let os = crate::zcrypto::node_platform_for(std::env::consts::OS);
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("{os}-{arch}")
}

pub const BUILTIN_CODING_PLAN_MODELS: &[&str] = &["GLM-5.3", "GLM-5.3-Flash"];
pub const BUILTIN_START_PLAN_MODELS: &[&str] = &["GLM-5.3-Flash", "GLM-5.2", "GLM-5-Turbo"];

pub(crate) fn canonical_model_id(id: &str) -> String {
    let lower = id.to_lowercase();
    for m in BUILTIN_CODING_PLAN_MODELS
        .iter()
        .chain(BUILTIN_START_PLAN_MODELS.iter())
    {
        if m.to_lowercase() == lower {
            return m.to_string();
        }
    }
    id.to_string()
}

const ZCODE_ORIGIN: &str = "https://zcode.z.ai";
pub(crate) const ZCODE_LANG: &str = "zh-CN";
const ZCODE_CHANNEL: &str = "stable";

pub(crate) fn device_mid() -> Option<String> {
    static CACHE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            let home = crate::store::pick_home(
                std::env::var("ZCODE_SWITCH_HOME").ok().map(std::path::PathBuf::from),
                std::env::var("USERPROFILE").ok().map(std::path::PathBuf::from),
                std::env::var("HOME").ok().map(std::path::PathBuf::from),
            );
            let p = crate::store::resolve_data_root(&home)
                .join(".zcode")
                .join("v2")
                .join("telemetry-state.json");
            std::fs::read_to_string(p)
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .and_then(|v| v.get("deviceMid").and_then(|m| m.as_str()).map(String::from))
        })
        .clone()
}

#[cfg(windows)]
pub(crate) fn os_version() -> Option<String> {
    static CACHE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            let out = no_window("reg")
                .args(["query", r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion", "/v", "CurrentBuildNumber"])
                .output()
                .ok()?;
            let txt = String::from_utf8_lossy(&out.stdout);
            let build = txt.lines().find(|l| l.contains("CurrentBuildNumber"))?
                .rsplit(' ').find(|t| !t.is_empty())?.to_string();
            Some(format!("10.0.{build}"))
        })
        .clone()
}

#[cfg(not(windows))]
pub(crate) fn os_version() -> Option<String> {
    static CACHE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            let out = no_window("uname").arg("-r").output().ok()?;
            let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
            (!v.is_empty()).then_some(v)
        })
        .clone()
}

#[cfg(windows)]
pub(crate) fn client_timezone() -> String {
    static CACHE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            let out = no_window("tzutil").arg("/g").output().ok();
            let name = out
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            match name.as_str() {
                "China Standard Time" | "China Daylight Time" => "Asia/Shanghai",
                "Singapore Standard Time" => "Asia/Singapore",
                "Tokyo Standard Time" => "Asia/Tokyo",
                "UTC" => "UTC",
                _ => "unknown",
            }
            .to_string()
        })
        .clone()
}

#[cfg(not(windows))]
pub(crate) fn client_timezone() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "unknown".to_string())
}

pub(crate) fn zcode_app_version() -> String {
    static CACHE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            for hive in [
                r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
                r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
                r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
            ] {
                let Ok(out) = no_window("reg").args(["query", hive, "/s"]).output() else {
                    continue;
                };
                let txt = String::from_utf8_lossy(&out.stdout);
                let (mut name, mut ver) = (String::new(), String::new());
                for line in txt.lines() {
                    let l = line.trim();
                    if l.starts_with("HKEY_") {
                        if is_zcode_display_name(&name) && !ver.is_empty() {
                            return normalize_version(&ver);
                        }
                        name.clear();
                        ver.clear();
                        continue;
                    }
                    if let Some(rest) = l.strip_prefix("DisplayName") {
                        name = rest.trim_start().trim_start_matches("REG_SZ").trim().to_string();
                    } else if let Some(rest) = l.strip_prefix("DisplayVersion") {
                        ver = rest.trim_start().trim_start_matches("REG_SZ").trim().to_string();
                    }
                }
                if is_zcode_display_name(&name) && !ver.is_empty() {
                    return normalize_version(&ver);
                }
            }
            CLIENT_APP_VERSION.to_string()
        })
        .clone()
}

fn is_zcode_display_name(name: &str) -> bool {
    let l = name.to_lowercase();
    l.contains("zcode") && !l.contains("switch")
}

fn normalize_version(v: &str) -> String {
    let parts: Vec<&str> = v.split('.').collect();
    if parts.len() >= 3 {
        format!("{}.{}.{}", parts[0], parts[1], parts[2])
    } else {
        v.to_string()
    }
}

pub(crate) fn zai_billing_headers(token: &str) -> Vec<(String, String)> {
    zai_billing_headers_with_mid(token, device_mid())
}

pub(crate) fn zai_billing_headers_with_mid(token: &str, mid: Option<String>) -> Vec<(String, String)> {
    zai_headers_with_version(zcode_app_version(), token, mid)
}

pub(crate) struct OAuthFlowHeaders(pub Vec<(String, String)>);

pub(crate) fn zai_oauth_headers_with_mid(token: &str, mid: Option<String>) -> OAuthFlowHeaders {
    OAuthFlowHeaders(zai_headers_with_version(CLIENT_APP_VERSION.to_string(), token, mid))
}

fn zai_headers_with_version(ver: String, token: &str, mid: Option<String>) -> Vec<(String, String)> {
    let mut h: Vec<(String, String)> = vec![
        ("User-Agent".into(), format!("ZCode/{ver}")),
        ("HTTP-Referer".into(), ZCODE_ORIGIN.into()),
        ("X-Title".into(), "Z Code@electron".into()),
        ("X-ZCode-App-Version".into(), ver.clone()),
        ("X-Platform".into(), client_platform()),
        ("X-Release-Channel".into(), ZCODE_CHANNEL.into()),
        ("X-Client-Language".into(), ZCODE_LANG.into()),
        ("X-Client-Timezone".into(), client_timezone()),
        ("X-Os-Category".into(), std::env::consts::OS.into()),
    ];
    if let Some(v) = os_version() {
        h.push(("X-Os-Version".into(), v));
    }
    if let Some(mid) = mid {
        h.push(("X-Device-Mid".into(), mid));
    }
    h.push(("Authorization".into(), format!("Bearer {token}")));
    h.push(("x-request-id".into(), uuid::Uuid::new_v4().to_string()));
    h
}

fn bigmodel_headers(token: &str) -> Vec<(String, String)> {
    vec![
        ("Authorization".into(), format!("Bearer {token}")),
        ("User-Agent".into(), format!("ZCode/{}", zcode_app_version())),
        ("x-request-id".into(), uuid::Uuid::new_v4().to_string()),
    ]
}

#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct QuotaItem {
    pub name: String,
    pub total: Option<f64>,
    pub used: Option<f64>,
    pub remaining: Option<f64>,
    pub percent_used: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_percentage: Option<f64>,
    pub unit: String,
    pub period_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct PlanSlot {
    #[serde(skip_serializing)]
    pub pid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expire: Option<String>,
    pub total: Option<f64>,
    pub used: Option<f64>,
    pub remaining: Option<f64>,
    pub percent_used: Option<f64>,
    pub items: Vec<QuotaItem>,
}

#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct QuotaOverview {
    pub total: Option<f64>,
    pub used: Option<f64>,
    pub remaining: Option<f64>,
    pub percent_used: Option<f64>,
    pub plan_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_expire: Option<String>,
    pub is_empty: bool,
    pub items: Vec<QuotaItem>,
    pub refreshed_at: i64,
    pub source: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub plans: Vec<PlanSlot>,
}

fn safe_decrypt(value: Option<&str>, secret: &str) -> Option<String> {
    let v = value?;
    if zcrypto::is_encrypted(v) {
        zcrypto::decrypt_with_secret(v, secret).ok()
    } else {
        Some(v.to_string())
    }
}

fn looks_like_token(v: &str) -> bool {
    v.trim().len() > 20
}

fn coding_plan_api_keys(config: Option<&Value>) -> Vec<String> {
    let mut keys = vec![];
    let providers = match config.and_then(|c| c.get("provider")).and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return keys,
    };
    let mut ordered: Vec<(&String, &Value)> = providers.iter().collect();
    ordered.sort_by_key(|(_, p)| if p.get("enabled").and_then(|e| e.as_bool()).unwrap_or(false) { 0 } else { 1 });
    for (id, p) in ordered {
        if !id.contains("coding-plan") {
            continue;
        }
        if let Some(k) = p.get("options").and_then(|o| o.get("apiKey")).and_then(|k| k.as_str()) {
            if !k.starts_with("enc:") && looks_like_token(k) && !keys.contains(&k.to_string()) {
                keys.push(k.to_string());
            }
        }
    }
    keys
}

fn coding_plan_keys_from_creds(creds: &Value, secret: &str) -> Vec<String> {
    let Some(map) = creds.as_object() else { return vec![] };
    let mut out: Vec<String> = vec![];
    for (k, v) in map {
        if !k.starts_with("account-provider:") || !k.ends_with(":api-key") { continue; }
        if !k.contains("coding-plan") { continue; }
        let Some(p) = v.as_str().and_then(|v| safe_decrypt(Some(v), secret)) else { continue };
        if looks_like_token(&p) && !out.contains(&p) { out.push(p); }
    }
    out
}

pub fn candidate_tokens(creds: &Value, config: Option<&Value>, secret: &str, new_gen: bool) -> Vec<String> {
    let mut tokens: Vec<String> = vec![];
    let add = |plain: Option<String>, tokens: &mut Vec<String>| {
        if let Some(p) = plain {
            if looks_like_token(&p) && !tokens.contains(&p) {
                tokens.push(p);
            }
        }
    };
    let creds_keys = coding_plan_keys_from_creds(creds, secret);
    if new_gen {
        for k in &creds_keys {
            add(Some(k.clone()), &mut tokens);
        }
    }
    for k in coding_plan_api_keys(config) {
        tokens.push(k);
    }
    let active = safe_decrypt(
        creds.get("oauth:active_provider").and_then(|v| v.as_str()),
        secret,
    )
    .unwrap_or_else(|| "zai".into());
    let map = creds.as_object();
    add(
        map.and_then(|m| m.get("zcodejwttoken")).and_then(|v| v.as_str()).and_then(|v| safe_decrypt(Some(v), secret)),
        &mut tokens,
    );
    for key in [
        format!("oauth:{active}:access_token"),
        "oauth:bigmodel:access_token".to_string(),
        "oauth:zai:access_token".to_string(),
    ] {
        add(
            map.and_then(|m| m.get(&key)).and_then(|v| v.as_str()).and_then(|v| safe_decrypt(Some(v), secret)),
            &mut tokens,
        );
    }
    if !new_gen {
        for k in creds_keys {
            add(Some(k), &mut tokens);
        }
    }
    tokens
}

fn http_get_json(url: &str, token: &str, retry_429: bool) -> Result<Value, String> {
    let retry_delays = [500u64, 1500, 4000];
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build();
    let headers = if url.contains("zcode.z.ai") {
        zai_billing_headers(token)
    } else {
        bigmodel_headers(token)
    };
    let mut last_err: Option<String> = None;
    let mut backoff: std::slice::Iter<'_, u64> = if retry_429 { retry_delays.iter() } else { [].iter() };
    loop {
        let mut req = agent.get(url);
        for (k, v) in &headers {
            req = req.set(k, v);
        }
        let resp = req.call();
        match resp {
            Ok(r) => {
                let text = r.into_string().map_err(|e| crate::i18n::trf("err.http.read", &[("e", &e.to_string())]))?;
                return Ok(if text.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_str(&text).unwrap_or(Value::String(text.clone()))
                });
            }
            Err(ureq::Error::Status(code, r)) => {
                let body = r.into_string().unwrap_or_default();
                if let Ok(v) = serde_json::from_str::<Value>(&body) {
                    if v.get("code").and_then(|c| c.as_i64()) == Some(401) {
                        return Err(crate::i18n::tr("err.token.biz401"));
                    }
                }
                if code == 429 {
                    match backoff.next() {
                        Some(d) => {
                            last_err = Some(crate::i18n::tr("err.quota.rate_limited"));
                            sleep(Duration::from_millis(*d));
                            continue;
                        }
                        None => return Err(last_err.unwrap_or_else(|| crate::i18n::tr("err.quota.http429"))),
                    }
                }
                if code == 401 || code == 403 {
                    return Err(crate::i18n::trf("err.token.http401", &[("code", &code.to_string())]));
                }
                let msg = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|v| ["message", "msg", "error"].iter().find_map(|k| v.get(k).and_then(|x| x.as_str()).map(String::from)));
                return Err(crate::i18n::trf("err.quota.http", &[("code", &code.to_string()), ("msg", &msg.unwrap_or_default())]));
            }
            Err(e) => return Err(crate::i18n::trf("err.network", &[("e", &e.to_string())])),
        }
    }
}

type FetchFn<'a> = dyn Fn(&str, &str) -> Result<Value, String> + 'a;

fn query_with_token_via(token: &str, fetch: &FetchFn) -> Result<QuotaOverview, String> {
    let mut best_err: Option<String>;

    match fetch(QUOTA_LIMIT_URL, token) {
        Ok(limit_resp) => {
            if business_ok(&limit_resp) {
                let sub = fetch(SUBSCRIPTION_URL, token).ok();
                let mut ov = normalize_quota_limit(&limit_resp, sub.as_ref());
                ov.refreshed_at = chrono::Local::now().timestamp_millis();
                ov.source = "bigmodel.cn/api/monitor".into();
                return Ok(ov);
            }
            let code = limit_resp.get("code").and_then(|c| c.as_i64());
            best_err = Some(match code {
                Some(401) => crate::i18n::tr("err.token.biz401"),
                Some(c) => {
                    let msg = ["msg", "message", "error"]
                        .iter()
                        .find_map(|k| limit_resp.get(k).and_then(|x| x.as_str()).map(String::from))
                        .unwrap_or_default();
                    crate::i18n::trf("err.quota.biz", &[("code", &c.to_string()), ("msg", &msg)])
                }
                None => crate::i18n::tr("err.quota.bad_resp"),
            });
        }
        Err(e) => best_err = Some(e),
    }

    let url = format!("{BILLING_BALANCE_URL}?app_version={}", zcode_app_version());
    match fetch(&url, token) {
        Ok(balance) if business_ok(&balance) => {
            let mut ov = normalize_balance(&balance);
            ov.refreshed_at = chrono::Local::now().timestamp_millis();
            ov.source = "zcode.z.ai/billing".into();
            return Ok(ov);
        }
        Ok(_) => {}
        Err(e) => {
            if best_err.is_none() {
                best_err = Some(e);
            }
        }
    }

    Err(best_err.unwrap_or_else(|| crate::i18n::tr("err.quota.fail")))
}

fn query_with_token(token: &str) -> Result<QuotaOverview, String> {
    query_with_token_via(token, &|url, tok| http_get_json(url, tok, true))
}

pub(crate) fn business_ok(v: &Value) -> bool {
    let code = v.get("code").and_then(|c| c.as_i64());
    let success = v.get("success").and_then(|s| s.as_bool());
    (code.is_none() || code == Some(200) || code == Some(0)) && success != Some(false)
}

pub(crate) fn is_active_coding_plan_entry(s: &Value) -> bool {
    let coding_name = ["productId", "productName"].iter().any(|k| {
        s.get(k)
            .and_then(|v| v.as_str())
            .map(|v| v.to_lowercase().contains("coding"))
            .unwrap_or(false)
    });
    if !coding_name {
        return false;
    }
    s.get("inCurrentPeriod").and_then(|v| v.as_bool()) == Some(true)
        && s.get("status").and_then(|v| v.as_str()) == Some("VALID")
}

pub fn query_quota(tokens: &[String]) -> Result<QuotaOverview, String> {
    if tokens.is_empty() {
        return Err(crate::i18n::tr("err.quota.no_token"));
    }
    let mut last_err: Option<String> = None;
    let mut first_business: Option<String> = None;
    let mut auth_fail = 0usize;
    for t in tokens {
        match query_with_token(t) {
            Ok(ov) => return Ok(ov),
            Err(e) => {
                if e.contains("401") {
                    auth_fail += 1;
                } else if first_business.is_none() {
                    first_business = Some(e.clone());
                }
                last_err = Some(e);
            }
        }
    }
    if auth_fail > 0 && auth_fail == tokens.len() {
        sleep(Duration::from_millis(1500));
        if let Ok(ov) = query_with_token(&tokens[0]) {
            return Ok(ov);
        }
        return Err(crate::i18n::tr("err.token.expired"));
    }
    Err(first_business.or(last_err).unwrap_or_else(|| crate::i18n::tr("err.quota.fail")))
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Channel {
    Monitor(String),
    ZaiBilling(String),
}

pub(crate) fn is_no_plan_message(msg: &str) -> bool {
    msg.contains("不存在coding plan") || msg.contains("没有资格")
}

pub(crate) fn zai_billing_token(creds: &Value, config: Option<&Value>, secret: &str) -> Option<String> {
    let jwt = safe_decrypt(creds.get("zcodejwttoken").and_then(|v| v.as_str()), secret);
    let active = safe_decrypt(
        creds.get("oauth:active_provider").and_then(|v| v.as_str()),
        secret,
    );
    let use_jwt = if active.as_deref() != Some("bigmodel") {
        jwt.is_some()
    } else {
        false
    };
    if use_jwt {
        return jwt;
    }
    let providers = config?.get("provider")?.as_object()?;
    let mut ordered: Vec<(&String, &Value)> = providers.iter().collect();
    ordered.sort_by_key(|(_, p)| if p.get("enabled").and_then(|e| e.as_bool()).unwrap_or(false) { 0 } else { 1 });
    for (id, p) in ordered {
        if id.contains("start-plan") {
            if let Some(k) = p.get("options").and_then(|o| o.get("apiKey")).and_then(|k| k.as_str()) {
                if !k.starts_with("enc:") && looks_like_token(k) {
                    return Some(k.to_string());
                }
            }
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum BizErrFamily { Auth, QuotaExhausted, RateLimited, SecurityReject, Param, Server, Unknown }

pub(crate) fn classify_biz_err(code: i64) -> BizErrFamily {
    use BizErrFamily::*;
    match code {
        401 | 1006 => Auth,
        1005 => QuotaExhausted,
        3002 | 3008 | 3009 | 3010 => RateLimited,
        3007 => SecurityReject,
        3001 | 3006 | 3102 => Param,
        2007 => Server,
        _ => Unknown,
    }
}

pub(crate) fn pick_channels(creds: &Value, config: Option<&Value>, secret: &str, new_gen: bool) -> Vec<Channel> {
    let mut chans: Vec<Channel> = vec![];
    if new_gen {
        for k in coding_plan_keys_from_creds(creds, secret) {
            if !chans.contains(&Channel::Monitor(k.clone())) {
                chans.push(Channel::Monitor(k));
            }
        }
        if let Some(t) = zai_billing_token(creds, config, secret) {
            if !chans.contains(&Channel::ZaiBilling(t.clone())) {
                chans.push(Channel::ZaiBilling(t));
            }
        }
    }
    let providers = match config.and_then(|c| c.get("provider")).and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return chans,
    };
    let mut ordered: Vec<(&String, &Value)> = providers.iter().collect();
    ordered.sort_by_key(|(_, p)| if p.get("enabled").and_then(|e| e.as_bool()).unwrap_or(false) { 0 } else { 1 });
    let provider_key = |p: &Value| -> Option<String> {
        let k = p.get("options").and_then(|o| o.get("apiKey")).and_then(|k| k.as_str())?;
        (!k.starts_with("enc:") && looks_like_token(k)).then(|| k.to_string())
    };
    for (id, p) in ordered {
        let key = provider_key(p);
        if id.contains("start-plan") {
            let jwt = safe_decrypt(
                creds.get("zcodejwttoken").and_then(|v| v.as_str()),
                secret,
            );
            let active = safe_decrypt(
                creds.get("oauth:active_provider").and_then(|v| v.as_str()),
                secret,
            );
            let use_jwt = if id.starts_with("builtin:zai") {
                jwt.is_some()
            } else {
                jwt.is_some() && active.as_deref() == Some("bigmodel")
            };
            let tok = if use_jwt { jwt } else { None }.or(key);
            if let Some(t) = tok {
                if !chans.contains(&Channel::ZaiBilling(t.clone())) {
                    chans.push(Channel::ZaiBilling(t));
                }
            }
        } else if id.contains("coding-plan") {
            if let Some(k) = key {
                if !chans.contains(&Channel::Monitor(k.clone())) {
                    chans.push(Channel::Monitor(k));
                }
            }
        }
    }
    if !new_gen {
        for k in coding_plan_keys_from_creds(creds, secret) {
            if !chans.contains(&Channel::Monitor(k.clone())) {
                chans.push(Channel::Monitor(k));
            }
        }
    }
    chans
}

fn no_plan_overview() -> QuotaOverview {
    QuotaOverview {
        plan_tier: None,
        is_empty: true,
        source: "no_plan".into(),
        ..Default::default()
    }
}

fn query_channels_via(channels: &[Channel], fetch: &FetchFn) -> Result<QuotaOverview, String> {
    let mut best_err: Option<String> = None;
    let mut saw_no_plan = false;
    let mut parts: Vec<QuotaOverview> = vec![];
    for ch in channels {
        match ch {
            Channel::Monitor(key) => match fetch(QUOTA_LIMIT_URL, key) {
                Ok(resp) => {
                    if business_ok(&resp) {
                        let sub = fetch(SUBSCRIPTION_URL, key).ok();
                        let mut ov = normalize_quota_limit(&resp, sub.as_ref());
                        ov.refreshed_at = chrono::Local::now().timestamp_millis();
                        ov.source = "bigmodel.cn/api/monitor".into();
                        parts.push(ov);
                    } else {
                        let msg = ["msg", "message", "error"]
                            .iter()
                            .find_map(|k| resp.get(k).and_then(|x| x.as_str()).map(String::from))
                            .unwrap_or_default();
                        if is_no_plan_message(&msg) {
                            saw_no_plan = true;
                        } else if best_err.is_none() {
                            let code = resp.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
                            best_err = Some(crate::i18n::trf("err.quota.biz", &[("code", &code.to_string()), ("msg", &msg)]));
                        }
                    }
                }
                Err(e) => {
                    if best_err.is_none() {
                        best_err = Some(e);
                    }
                }
            },
            Channel::ZaiBilling(tok) => {
                let url = format!("{BILLING_BALANCE_URL}?app_version={}", zcode_app_version());
                match fetch(&url, tok) {
                    Ok(balance) if business_ok(&balance) => {
                        let mut ov = normalize_balance(&balance);
                        ov.refreshed_at = chrono::Local::now().timestamp_millis();
                        ov.source = "zcode.z.ai/billing".into();
                        let has_plan = ov.plan_tier.is_some() || !ov.plans.is_empty();
                        if has_plan {
                            parts.push(ov);
                        } else {
                            saw_no_plan = true;
                        }
                    }
                    Ok(resp) => {
                        if best_err.is_none() {
                            let msg = ["msg", "message", "error"]
                                .iter()
                                .find_map(|k| resp.get(k).and_then(|x| x.as_str()).map(String::from))
                                .unwrap_or_default();
                            let code = resp.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
                            best_err = Some(crate::i18n::trf("err.quota.biz", &[("code", &code.to_string()), ("msg", &msg)]));
                        }
                    }
                    Err(e) => {
                        if best_err.is_none() {
                            best_err = Some(e);
                        }
                    }
                }
            }
        }
    }
    if parts.is_empty() {
        if saw_no_plan {
            return Ok(no_plan_overview());
        }
        return Err(best_err.unwrap_or_else(|| crate::i18n::tr("err.quota.fail")));
    }
    Ok(merge_parts(parts))
}

fn merge_parts(parts: Vec<QuotaOverview>) -> QuotaOverview {
    let mut slots: Vec<PlanSlot> = vec![];
    let mut slot_src: Vec<String> = vec![];
    let mut sources: Vec<&str> = vec![];
    let mut refreshed = 0i64;
    for p in &parts {
        if !sources.contains(&p.source.as_str()) {
            sources.push(&p.source);
        }
        refreshed = refreshed.max(p.refreshed_at);
        for s in &p.plans {
            let dup = slot_src.iter().enumerate().any(|(i, src)| {
                src != &p.source
                    && slots[i].tier == s.tier
                    && slots[i].name == s.name
                    && !slots[i].items.is_empty()
                    && !s.items.is_empty()
            });
            if !dup {
                slots.push(s.clone());
                slot_src.push(p.source.clone());
            }
        }
    }
    let items: Vec<QuotaItem> = slots.iter().flat_map(|s| s.items.iter().cloned()).collect();
    let content = |s: &PlanSlot| s.tier.is_some() || !s.items.is_empty() || s.total.is_some();
    let mut pri_idx: Option<usize> = None;
    for (i, s) in slots.iter().enumerate() {
        if !content(s) {
            continue;
        }
        pri_idx = match pri_idx {
            None => Some(i),
            Some(p) if tier_rank(s.tier_code.as_deref()) > tier_rank(slots[p].tier_code.as_deref()) => Some(i),
            _ => pri_idx,
        };
    }
    let (total, used, remaining, percent_used) = match pri_idx.map(|i| &slots[i]) {
        Some(p) => (p.total, p.used, p.remaining, p.percent_used),
        None => (None, None, None, None),
    };
    QuotaOverview {
        total,
        used,
        remaining,
        percent_used,
        plan_tier: pri_idx.map(|i| slots[i].tier.clone()).flatten(),
        plan_expire: pri_idx.map(|i| slots[i].expire.clone()).flatten(),
        is_empty: slots.is_empty(),
        items,
        refreshed_at: refreshed,
        source: sources.join(" + "),
        plans: slots,
    }
}

fn query_channels(channels: &[Channel]) -> Result<QuotaOverview, String> {
    query_channels_via(channels, &|url, tok| http_get_json(url, tok, true))
}

pub fn quota_for_live(home: &Path, creds: &Value, config: Option<&Value>) -> Result<QuotaOverview, String> {
    let secret = zcrypto::default_secret(home);
    let new_gen = crate::store::new_gen_provider_config(home);
    let channels = pick_channels(creds, config, &secret, new_gen);
    if !channels.is_empty() {
        if let Ok(ov) = query_channels(&channels) {
            return Ok(ov);
        }
    }
    let tokens = candidate_tokens(creds, config, &secret, new_gen);
    query_quota(&tokens)
}

pub fn quota_for_snapshot(home: &Path, creds: &Value, config: Option<&Value>) -> Result<QuotaOverview, String> {
    quota_for_live(home, creds, config)
}

fn unit_label(unit: Option<i64>, number: Option<i64>) -> (String, String) {
    match unit {
        Some(3) => {
            let n = number.unwrap_or(5);
            (format!("每 {n} 小时"), format!("hours:{n}"))
        }
        Some(4) => ("每天".into(), "daily".into()),
        Some(5) => ("每月".into(), "monthly".into()),
        Some(6) => ("每周".into(), "weekly".into()),
        _ => ("每周期".into(), "cycle".into()),
    }
}

fn fmt_reset_time(ms: Option<i64>) -> Option<String> {
    let ms = ms?;
    if ms <= 0 {
        return None;
    }
    use chrono::TimeZone;
    Some(chrono::Local.timestamp_millis_opt(ms).single()?.format("%m-%d %H:%M").to_string())
}

fn safe_prefix(s: &str, n: usize) -> &str {
    if s.len() <= n {
        return s;
    }
    let mut end = n;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn safe_suffix(s: &str, n: usize) -> &str {
    if s.len() <= n {
        return s;
    }
    let mut start = s.len() - n;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

fn looks_like_date(d: &str) -> bool {
    d.len() == 10 && d.as_bytes().get(4) == Some(&b'-') && d.as_bytes().get(7) == Some(&b'-')
}

fn looks_like_dt(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 16 {
        return false;
    }
    let p = &b[..16];
    p[4] == b'-' && p[7] == b'-' && p[10] == b' ' && p[13] == b':'
        && (0..16).all(|i| match i {
            0..=3 | 5..=6 | 8..=9 | 11..=12 | 14..=15 => p[i].is_ascii_digit(),
            _ => true,
        })
}

fn extract_expire(obj: &Value) -> Option<String> {
    const KEYS: [&str; 12] = [
        "nextRenewTime", "expireTime", "expire_time", "endTime", "end_time", "expireAt", "expiredTime", "validEndTime",
        "expires_at", "expiresAt", "expired_at", "period_end",
    ];
    for k in KEYS {
        let Some(v) = obj.get(k) else { continue };
        if let Some(n) = v.as_i64() {
            use chrono::TimeZone;
            if n > 1_000_000_000_000 {
                return chrono::Local.timestamp_millis_opt(n).single().map(|t| t.format("%Y-%m-%d %H:%M").to_string());
            }
            if n > 1_000_000_000 {
                return chrono::Local.timestamp_opt(n, 0).single().map(|t| t.format("%Y-%m-%d %H:%M").to_string());
            }
        }
        if let Some(s) = v.as_str() {
            let t = s.trim();
            if t.is_empty() {
                continue;
            }
            if let Ok(n) = t.parse::<i64>() {
                use chrono::TimeZone;
                if n > 1_000_000_000_000 {
                    if let Some(dt) = chrono::Local.timestamp_millis_opt(n).single() {
                        return Some(dt.format("%Y-%m-%d %H:%M").to_string());
                    }
                } else if n > 1_000_000_000 {
                    if let Some(dt) = chrono::Local.timestamp_opt(n, 0).single() {
                        return Some(dt.format("%Y-%m-%d %H:%M").to_string());
                    }
                }
            }
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(t) {
                return Some(dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string());
            }
            let t = if t.as_bytes().get(10) == Some(&b'T') {
                format!("{} {}", &t[..10], &t[11..])
            } else {
                t.to_string()
            };
            let d = safe_prefix(&t, 10);
            if looks_like_date(d) {
                if looks_like_dt(&t) {
                    return Some(safe_prefix(&t, 16).to_string());
                }
                return Some(d.to_string());
            }
            return Some(t.to_string());
        }
    }
    if let Some(s) = obj.get("valid").and_then(|v| v.as_str()) {
        let tail = safe_suffix(s, 19);
        if looks_like_dt(tail) {
            return Some(safe_prefix(tail, 16).to_string());
        }
        if looks_like_date(safe_prefix(tail, 10)) {
            return Some(safe_prefix(tail, 10).to_string());
        }
        let d = safe_prefix(s, 10);
        if looks_like_date(d) {
            return Some(d.to_string());
        }
    }
    None
}

fn expiry_field(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_string())
        }
        Value::Number(_) => {
            let mut o = serde_json::Map::new();
            o.insert("expires_at".to_string(), v.clone());
            extract_expire(&Value::Object(o))
        }
        _ => None,
    }
}

fn tier_from_level(level: &str) -> String {
    let l = level.to_lowercase();
    if l.contains("max") {
        "Max".into()
    } else if l.contains("pro") {
        "Pro".into()
    } else if l.contains("lite") {
        "Lite".into()
    } else {
        level.to_string()
    }
}

fn normalize_quota_limit(limit_resp: &Value, sub_resp: Option<&Value>) -> QuotaOverview {
    let data = limit_resp.get("data").cloned().unwrap_or(Value::Null);
    let limits = data.get("limits").and_then(|l| l.as_array()).cloned().unwrap_or_default();

    let mut items = vec![];
    let mut main: Option<QuotaItem> = None;
    for l in &limits {
        let typ = l.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let unit = l.get("unit").and_then(|u| u.as_i64());
        let number = l.get("number").and_then(|n| n.as_i64());
        let total = l.get("usage").and_then(|v| v.as_f64());
        let used = l.get("currentValue").and_then(|v| v.as_f64());
        let remaining = l.get("remaining").and_then(|v| v.as_f64());
        let percentage = l.get("percentage").and_then(|v| v.as_f64());
        let reset_ms = l.get("nextResetTime").and_then(|v| v.as_i64());
        let (period, window) = unit_label(unit, number);
        let (kind, name, unit_str, unit_code) = match typ {
            "TOKENS_LIMIT" => ("prompt_count", format!("提示次数（{period}）"), "次".to_string(), "count"),
            "TIME_LIMIT" => ("duration", format!("使用时长（{period}）"), "分钟".to_string(), "minutes"),
            _ => ("raw", format!("{typ}（{period}）"), String::new(), ""),
        };
        let percent_used = if let (Some(t), Some(u)) = (total, used) {
            if t > 0.0 { Some((u / t * 100.0).clamp(0.0, 100.0)) } else { None }
        } else {
            percentage.map(|p| p.clamp(0.0, 100.0))
        };
        let reset = fmt_reset_time(reset_ms);
        let item = QuotaItem {
            name,
            total,
            used,
            remaining,
            percent_used,
            server_percentage: percentage,
            unit: unit_str,
            period_end: reset.as_ref().map(|r| format!("{r} 重置")),
            kind: (kind != "").then(|| kind.to_string()),
            window: Some(window),
            unit_code: (unit_code != "").then(|| unit_code.to_string()),
            reset,
        };
        if typ == "TIME_LIMIT" && total.is_some() {
            main = main.or(Some(item.clone()));
        }
        items.push(item);
    }

    let level = data.get("level").and_then(|l| l.as_str()).map(String::from);
    let mut plan_tier = level.as_deref().map(tier_from_level);
    let mut plan_expire: Option<String> = None;
    let mut product_name: Option<String> = None;
    if let Some(sub) = sub_resp {
        if business_ok(sub) {
            if std::env::var("ZSW_DUMP_SUB").is_ok() {
                eprintln!("[zsw] subscription/list raw: {sub}");
            }
            if let Some(arr) = sub.get("data").and_then(|d| d.as_array()) {
                let current = arr
                    .iter()
                    .find(|s| is_active_coding_plan_entry(s))
                    .or_else(|| arr.first());
                if let Some(s) = current {
                    if let Some(pn) = s.get("productName").and_then(|x| x.as_str()) {
                        if !pn.trim().is_empty() {
                            product_name = Some(pn.to_string());
                            plan_tier = Some(tier_from_level(pn));
                        }
                    }
                    plan_expire = extract_expire(s);
                }
            }
        }
    }

    let main = main.or_else(|| items.iter().find(|i| i.total.is_some()).cloned());
    let (total, used, remaining, percent_used) = match &main {
        Some(m) => (m.total, m.used, m.remaining, m.percent_used),
        None => {
            let p = items.iter().find_map(|i| i.percent_used);
            (None, None, None, p)
        }
    };

    let plans = if plan_tier.is_some() || !items.is_empty() {
        vec![PlanSlot {
            pid: String::new(),
            tier: plan_tier.clone(),
            tier_code: plan_tier.as_deref().map(tier_code_from_display),
            name: product_name,
            expire: plan_expire.clone(),
            total,
            used,
            remaining,
            percent_used,
            items: items.clone(),
        }]
    } else {
        vec![]
    };

    QuotaOverview {
        total,
        used,
        remaining,
        percent_used,
        plan_tier,
        plan_expire,
        is_empty: limits.is_empty(),
        items,
        refreshed_at: 0,
        source: String::new(),
        plans,
    }
}

fn unwrap(data: &Value) -> Value {
    let mut cur = data.clone();
    for _ in 0..4 {
        if !cur.is_object() {
            return cur;
        }
        if let Some(d) = cur.get("data") {
            cur = d.clone();
            continue;
        }
        if let Some(r) = cur.get("result") {
            cur = r.clone();
            continue;
        }
        break;
    }
    cur
}

fn flatten_numbers(obj: &Value, prefix: &str, out: &mut Vec<(String, f64)>) {
    if let Some(m) = obj.as_object() {
        for (k, v) in m {
            let p = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
            if let Some(n) = to_number(v) {
                out.push((p, n));
            } else {
                flatten_numbers(v, &p, out);
            }
        }
    } else if let Some(arr) = obj.as_array() {
        for (i, v) in arr.iter().enumerate() {
            let p = format!("{prefix}.{i}");
            if let Some(n) = to_number(v) {
                out.push((p, n));
            } else {
                flatten_numbers(v, &p, out);
            }
        }
    }
}

fn to_number(v: &Value) -> Option<f64> {
    if let Some(n) = v.as_f64() {
        return if n.is_finite() { Some(n) } else { None };
    }
    if let Some(s) = v.as_str() {
        let t = s.replace(',', "");
        return t.trim().parse::<f64>().ok();
    }
    None
}

fn sum_numbers(pool: &[(String, f64)], keys: &[&str]) -> Option<f64> {
    let mut total = 0.0;
    let mut count = 0;
    for (path, v) in pool {
        let name = path.rsplit('.').next().unwrap_or("");
        if keys.contains(&name) {
            total += v;
            count += 1;
        }
    }
    if count > 0 { Some(total) } else { None }
}

fn first_number(pool: &[(String, f64)], keys: &[&str]) -> Option<f64> {
    for (path, v) in pool {
        let name = path.rsplit('.').next().unwrap_or("");
        if keys.contains(&name) {
            return Some(*v);
        }
    }
    None
}

pub fn extract_plan_tier(current_data: &Value) -> Option<String> {
    let cur = unwrap(current_data);
    let plans = cur.get("plans").and_then(|p| p.as_array())?;
    let active: Vec<&Value> = plans
        .iter()
        .filter(|p| p.get("status").and_then(|s| s.as_str()).unwrap_or("").to_lowercase() == "active")
        .collect();
    if active.is_empty() {
        return None;
    }
    let matches = |kw: &[&str]| {
        active.iter().any(|p| {
            let id = p.get("plan_id").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            kw.iter().any(|k| id.contains(k) || name.contains(k))
        })
    };
    if matches(&["max"]) {
        Some("Max".into())
    } else if matches(&["pro"]) {
        Some("Pro".into())
    } else if matches(&["lite"]) {
        Some("Lite".into())
    } else if matches(&["start-plan", "start plan", "start"]) {
        Some("Start Plan".into())
    } else {
        None
    }
}

fn plan_tier_from_id(plan_id: &str, name: Option<&str>) -> (String, String) {
    let mut hay = plan_id.to_lowercase();
    if let Some(n) = name {
        hay.push(' ');
        hay.push_str(&n.to_lowercase());
    }
    if hay.contains("max") {
        ("Max".into(), "max".into())
    } else if hay.contains("pro") {
        ("Pro".into(), "pro".into())
    } else if hay.contains("lite") {
        ("Lite".into(), "lite".into())
    } else if hay.contains("start") {
        ("Start Plan".into(), "start".into())
    } else if ["trial", "taste", "experience", "gift", "weekend", "promo", "activity", "体验"]
        .iter()
        .any(|k| hay.contains(k))
    {
        ("体验".into(), "trial".into())
    } else {
        (plan_id.to_string(), "other".into())
    }
}

fn tier_code_from_display(tier: &str) -> String {
    let t = tier.to_lowercase();
    if t.contains("max") {
        "max".into()
    } else if t.contains("pro") {
        "pro".into()
    } else if t.contains("lite") {
        "lite".into()
    } else if t.contains("start") {
        "start".into()
    } else if t.contains("trial") || tier.contains("体验") {
        "trial".into()
    } else {
        "other".into()
    }
}

fn tier_rank(code: Option<&str>) -> u8 {
    match code.unwrap_or("") {
        "max" => 5,
        "pro" => 4,
        "lite" => 3,
        "start" => 2,
        "trial" => 1,
        _ => 0,
    }
}

fn normalize_balance(balance_data: &Value) -> QuotaOverview {
    let balance = unwrap(balance_data);
    if std::env::var("ZSW_DUMP_SUB").is_ok() {
        eprintln!("[zsw] billing/balance raw: {balance_data}");
    }
    let mut pool = vec![];
    flatten_numbers(&balance, "", &mut pool);

    let mut slots: Vec<PlanSlot> = balance
        .get("plans")
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|pl| {
                    pl.get("status")
                        .and_then(|s| s.as_str())
                        .map(|s| s.eq_ignore_ascii_case("active"))
                        .unwrap_or(false)
                })
                .map(|pl| {
                    let pid = pl.get("plan_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let pname = pl.get("name").and_then(|v| v.as_str()).map(str::to_string);
                    let (tier, tier_code) = plan_tier_from_id(&pid, pname.as_deref());
                    PlanSlot {
                        pid: pid.clone(),
                        tier: Some(tier),
                        tier_code: Some(tier_code),
                        name: Some(pname.filter(|s| !s.trim().is_empty()).unwrap_or(pid)),
                        expire: extract_expire(pl),
                        ..Default::default()
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let mut loose: Vec<QuotaItem> = vec![];
    let any_pid = balance
        .get("balances")
        .and_then(|b| b.as_array())
        .map(|arr| {
            arr.iter().any(|item| {
                ["plan_id", "planId", "entitlement_id"]
                    .iter()
                    .any(|k| item.get(k).and_then(Value::as_str).map(|s| !s.is_empty()).unwrap_or(false))
            })
        })
        .unwrap_or(false);
    if let Some(arr) = balance.get("balances").and_then(|b| b.as_array()) {
        for item in arr {
            let it_total = item.get("total_units").and_then(to_number);
            let it_used = item.get("used_units").and_then(to_number);
            let it_remaining = item
                .get("remaining_units")
                .and_then(to_number)
                .or_else(|| item.get("available_units").and_then(to_number));
            let it = QuotaItem {
                name: ["show_name", "name", "entitlement_id", "plan_id"]
                    .iter()
                    .find_map(|k| item.get(k).and_then(Value::as_str))
                    .unwrap_or("Unknown")
                    .to_string(),
                total: it_total,
                used: it_used,
                remaining: it_remaining,
                percent_used: match (it_total, it_used) {
                    (Some(t), Some(u)) if t > 0.0 => Some((u / t * 100.0).clamp(0.0, 100.0)),
                    _ => None,
                },
                server_percentage: None,
                unit: item.get("unit_type").or_else(|| item.get("meter")).and_then(Value::as_str).unwrap_or("quota").to_string(),
                period_end: ["period_end", "expires_at"].iter().find_map(|k| item.get(k).and_then(expiry_field)),
                ..Default::default()
            };
            let bpid = ["plan_id", "planId", "entitlement_id"]
                .iter()
                .find_map(|k| item.get(k).and_then(Value::as_str))
                .unwrap_or("");
            let target = if !bpid.is_empty() {
                slots.iter_mut().find(|s| s.pid == bpid)
            } else if slots.len() == 1 && !any_pid {
                slots.first_mut()
            } else {
                None
            };
            match target {
                Some(s) => {
                    if s.expire.is_none() {
                        s.expire = extract_expire(item);
                    }
                    s.items.push(it);
                }
                None => loose.push(it),
            }
        }
    }

    if slots.is_empty() {
        if !loose.is_empty() {
            let t = extract_plan_tier(balance_data);
            slots.push(PlanSlot {
                tier_code: t.as_deref().map(tier_code_from_display),
                tier: t,
                items: std::mem::take(&mut loose),
                ..Default::default()
            });
        } else {
            let ptot = sum_numbers(&pool, &["total_units"]).or_else(|| {
                first_number(&pool, &["total", "totalQuota", "totalCredits", "quotaTotal", "amountTotal", "creditTotal"])
            });
            let pused = sum_numbers(&pool, &["used_units"]).or_else(|| {
                first_number(&pool, &["used", "usedQuota", "usedCredits", "quotaUsed", "amountUsed", "consumed", "totalUsed"])
            });
            let prem = sum_numbers(&pool, &["remaining_units"]).or_else(|| {
                first_number(&pool, &["remaining", "remain", "balance", "available", "availableQuota", "left", "quotaRemaining"])
            });
            if ptot.is_some() || pused.is_some() || prem.is_some() {
                let t = extract_plan_tier(balance_data);
                slots.push(PlanSlot {
                    tier_code: t.as_deref().map(tier_code_from_display),
                    tier: t,
                    total: ptot,
                    used: pused,
                    remaining: prem,
                    ..Default::default()
                });
            }
        }
    } else if !loose.is_empty() {
        slots.push(PlanSlot {
            name: Some("其他额度".into()),
            tier_code: Some("other".into()),
            items: loose,
            ..Default::default()
        });
    }

    for s in &mut slots {
        let sum = |f: fn(&QuotaItem) -> Option<f64>| -> Option<f64> {
            let vals: Vec<f64> = s.items.iter().filter_map(f).collect();
            (!vals.is_empty()).then(|| vals.iter().sum())
        };
        if s.items.is_empty() && s.total.is_none() {
            continue;
        }
        s.total = sum(|i| i.total).or(s.total);
        s.used = sum(|i| i.used).or(s.used);
        s.remaining = sum(|i| i.remaining).or(s.remaining);
        if s.total.is_none() {
            if let (Some(u), Some(r)) = (s.used, s.remaining) {
                s.total = Some(u + r);
            }
        }
        if s.used.is_none() {
            if let (Some(t), Some(r)) = (s.total, s.remaining) {
                s.used = Some((t - r).max(0.0));
            }
        }
        if s.remaining.is_none() {
            if let (Some(t), Some(u)) = (s.total, s.used) {
                s.remaining = Some((t - u).max(0.0));
            }
        }
        if s.percent_used.is_none() {
            s.percent_used = match (s.total, s.used) {
                (Some(t), Some(u)) if t > 0.0 => Some((u / t * 100.0).clamp(0.0, 100.0)),
                _ => None,
            };
        }
    }
    let mut pri_idx = 0usize;
    for (i, s) in slots.iter().enumerate() {
        if tier_rank(s.tier_code.as_deref()) > tier_rank(slots[pri_idx].tier_code.as_deref()) {
            pri_idx = i;
        }
    }

    let items: Vec<QuotaItem> = slots.iter().flat_map(|s| s.items.iter().cloned()).collect();

    let plan_expire_chain = extract_expire(&balance)
        .or_else(|| {
            balance.get("plans").and_then(|p| p.as_array()).and_then(|arr| {
                arr.iter()
                    .find(|pl| pl.get("status").and_then(|s| s.as_str()).map(|s| s.eq_ignore_ascii_case("active")).unwrap_or(false))
                    .and_then(extract_expire)
                    .or_else(|| arr.first().and_then(extract_expire))
            })
        })
        .or_else(|| {
            balance.get("balances").and_then(|b| b.as_array()).and_then(|arr| {
                arr.iter()
                    .filter_map(|it| it.get("expires_at").and_then(|v| v.as_i64()))
                    .filter(|n| *n > 1_000_000_000)
                    .max()
                    .and_then(|n| {
                        use chrono::TimeZone;
                        chrono::Local.timestamp_opt(n, 0).single().map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                    })
            })
        })
        .or_else(|| items.iter().find_map(|i| i.period_end.clone().filter(|s| !s.is_empty())));

    if slots.len() == 1 && slots[0].expire.is_none() {
        slots[0].expire = plan_expire_chain.clone();
    }
    let (total, used, remaining, percent_used) = match slots.get(pri_idx) {
        Some(p) => (p.total, p.used, p.remaining, p.percent_used),
        None => {
            let p = items.iter().find_map(|i| i.percent_used);
            (None, None, None, p)
        }
    };

    QuotaOverview {
        total,
        used,
        remaining,
        percent_used,
        plan_tier: slots.get(pri_idx).and_then(|p| p.tier.clone()),
        plan_expire: slots.get(pri_idx).and_then(|p| p.expire.clone()).or(plan_expire_chain),
        is_empty: balance.get("balances").and_then(|b| b.as_array()).map(|a| a.is_empty()).unwrap_or(false),
        items,
        refreshed_at: 0,
        source: String::new(),
        plans: slots,
    }
}
