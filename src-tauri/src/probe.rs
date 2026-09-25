
use crate::i18n::tr;
use crate::quota::{self, Channel};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProbeOutcome {
    Entitled { entitlements: usize, models: Vec<String> },
    NoEntitlement,
    AuthFailed,
}

#[derive(Debug, Clone, PartialEq)]
enum ProbeErr {
    Auth,
    Other(String),
}

type ProbeFetch<'a> = dyn Fn(&str, &str) -> Result<Value, ProbeErr> + 'a;

pub(crate) fn switch_probe(
    creds: &Value,
    config: Option<&Value>,
    secret: &str,
    mid: Option<String>,
    new_gen: bool,
) -> Option<ProbeOutcome> {
    let channels = quota::pick_channels(creds, config, secret, new_gen);
    classify_via(&channels, &|url, token| real_fetch(url, token, mid.as_deref()))
}

pub(crate) fn probe_message(outcome: &ProbeOutcome) -> String {
    match outcome {
        ProbeOutcome::Entitled { .. } => String::new(),
        ProbeOutcome::NoEntitlement => tr("probe.no_entitlement"),
        ProbeOutcome::AuthFailed => tr("probe.auth_failed"),
    }
}

fn balance_entitlements(balance: &Value) -> Option<usize> {
    let data = balance.get("data").unwrap_or(balance);
    let data = data.as_object()?;
    let balances = data.get("balances").and_then(|b| b.as_array());
    let plans = data.get("plans").and_then(|p| p.as_array());
    if balances.is_none() && plans.is_none() {
        return None;
    }
    let n = balances
        .map(|a| a.len())
        .filter(|n| *n > 0)
        .or_else(|| {
            plans.map(|arr| {
                arr.iter()
                    .filter(|pl| {
                        pl.get("status")
                            .and_then(|s| s.as_str())
                            .map(|s| s.eq_ignore_ascii_case("active"))
                            .unwrap_or(false)
                    })
                    .count()
            })
        })?;
    Some(n)
}

fn balance_models(balance: &Value) -> Option<Vec<String>> {
    let data = balance.get("data").unwrap_or(balance);
    let arr = data.as_object()?.get("balances")?.as_array()?;
    let mut out: Vec<String> = vec![];
    for b in arr {
        let mut got: Vec<String> = vec![];
        if let Some(cs) = b.get("capabilities").and_then(|c| c.as_array()) {
            for c in cs.iter().filter_map(|c| c.as_str()) {
                let t = c.trim();
                if t.to_lowercase().starts_with("model:") {
                    let id = t["model:".len()..].trim();
                    if !id.is_empty() {
                        got.push(id.to_string());
                    }
                }
            }
        }
        if got.is_empty() {
            if let Some(n) = b
                .get("show_name")
                .and_then(|n| n.as_str())
                .map(str::trim)
                .filter(|n| !n.is_empty())
            {
                got.push(n.to_string());
            }
        }
        for m in got {
            let m = quota::canonical_model_id(&m);
            if !out.contains(&m) {
                out.push(m);
            }
        }
    }
    Some(out)
}

fn has_active_subscription(sub: &Value) -> bool {
    let arr = match sub.get("data").and_then(|d| d.as_array()) {
        Some(a) => a,
        None => match sub.as_array() {
            Some(a) => a,
            None => return false,
        },
    };
    arr.iter().any(quota::is_active_coding_plan_entry)
}

fn biz_code(v: &Value) -> Option<i64> {
    v.get("code").and_then(|c| c.as_i64())
}

fn envelope_msg(v: &Value) -> &str {
    ["msg", "message", "error"]
        .iter()
        .find_map(|k| v.get(k).and_then(|x| x.as_str()))
        .unwrap_or("")
}

enum Verdict {
    Entitled(usize, Vec<String>),
    Empty,
    Auth,
    Inconclusive,
}

fn monitor_verdict(v: Result<Value, ProbeErr>) -> Verdict {
    match v {
        Ok(v) if v.is_object() && quota::business_ok(&v) => {
            if has_active_subscription(&v) {
                Verdict::Entitled(1, vec![])
            } else {
                Verdict::Empty
            }
        }
        Ok(v) => {
            if quota::is_no_plan_message(envelope_msg(&v)) {
                Verdict::Empty
            } else if let Some(code) = biz_code(&v) {
                match quota::classify_biz_err(code) {
                    quota::BizErrFamily::Auth => Verdict::Auth,
                    quota::BizErrFamily::QuotaExhausted => Verdict::Empty,
                    _ => Verdict::Inconclusive,
                }
            } else {
                Verdict::Inconclusive
            }
        }
        Err(ProbeErr::Auth) => Verdict::Auth,
        Err(ProbeErr::Other(_)) => Verdict::Inconclusive,
    }
}

fn billing_verdict(v: Result<Value, ProbeErr>) -> Verdict {
    match v {
        Ok(v) if v.is_object() && quota::business_ok(&v) => match balance_entitlements(&v) {
            Some(n) if n > 0 => Verdict::Entitled(n, balance_models(&v).unwrap_or_default()),
            Some(_) => Verdict::Empty,
            None => Verdict::Inconclusive,
        },
        Ok(v) => {
            match biz_code(&v).map(quota::classify_biz_err) {
                Some(quota::BizErrFamily::Auth) => Verdict::Auth,
                Some(quota::BizErrFamily::QuotaExhausted) => Verdict::Empty,
                _ => Verdict::Inconclusive,
            }
        }
        Err(ProbeErr::Auth) => Verdict::Auth,
        Err(ProbeErr::Other(_)) => Verdict::Inconclusive,
    }
}

fn classify_via(channels: &[Channel], fetch: &ProbeFetch) -> Option<ProbeOutcome> {
    if channels.is_empty() {
        return None;
    }
    let mut entitled = 0usize;
    let mut models: Vec<String> = vec![];
    let mut definite_empty = false;
    let mut auth_failed = false;
    let mut definite_channels = 0usize;
    for ch in channels {
        let verdict = match ch {
            Channel::Monitor(key) => monitor_verdict(fetch(&quota::SUBSCRIPTION_URL, key)),
            Channel::ZaiBilling(tok) => {
                let url = format!("{}?app_version={}", quota::BILLING_BALANCE_URL, quota::zcode_app_version());
                billing_verdict(fetch(&url, tok))
            }
        };
        match verdict {
            Verdict::Entitled(n, ms) => {
                entitled += n;
                for m in ms {
                    if !models.contains(&m) {
                        models.push(m);
                    }
                }
                definite_channels += 1;
            }
            Verdict::Empty => {
                definite_empty = true;
                definite_channels += 1;
            }
            Verdict::Auth => {
                auth_failed = true;
                definite_channels += 1;
            }
            Verdict::Inconclusive => {}
        }
    }
    if entitled > 0 {
        return Some(ProbeOutcome::Entitled { entitlements: entitled, models });
    }
    if definite_channels == channels.len() && definite_empty && !auth_failed {
        return Some(ProbeOutcome::NoEntitlement);
    }
    if auth_failed {
        return Some(ProbeOutcome::AuthFailed);
    }
    None
}

fn real_fetch(url: &str, token: &str, mid: Option<&str>) -> Result<Value, ProbeErr> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout(Duration::from_secs(8))
        .build();
    let mut req = agent.get(url);
    if url.contains("zcode.z.ai") {
        for (k, v) in quota::zai_billing_headers_with_mid(token, mid.map(str::to_string)) {
            req = req.set(&k, &v);
        }
    } else {
        req = req
            .set("Authorization", &format!("Bearer {token}"))
            .set("User-Agent", &format!("ZCode/{}", quota::zcode_app_version()))
            .set("x-request-id", &uuid::Uuid::new_v4().to_string());
    }
    let resp = req.call().map_err(map_http_err)?;
    let text = resp
        .into_string()
        .map_err(|e| ProbeErr::Other(e.to_string()))?;
    if text.is_empty() {
        return Ok(Value::Null);
    }
    Ok(serde_json::from_str(&text).unwrap_or(Value::String(text.clone())))
}

fn map_http_err(e: ureq::Error) -> ProbeErr {
    match e {
        ureq::Error::Status(code, r) => {
            let body = r.into_string().unwrap_or_default();
            if code == 401 || code == 403 {
                return ProbeErr::Auth;
            }
            if let Ok(v) = serde_json::from_str::<Value>(&body) {
                if v.get("code").and_then(|c| c.as_i64()) == Some(401) {
                    return ProbeErr::Auth;
                }
            }
            ProbeErr::Other(format!("http {code}"))
        }
        other => ProbeErr::Other(other.to_string()),
    }
}
