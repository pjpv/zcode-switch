//! 上游请求组装：官方 ZCode 客户端形态的头族与 body 变换。
//!
//! 3012 对话风控的三要素中，本模块承载前两个（验证码 param 为第三要素，MVP
//! 无头求解缺失，收到 3007 时冷却换号）：
//!   1. 官方 system 提示词注入（zcode_system.json，与版本头同版校验）
//!   2. JWT 恒定派生的 device-mid / session 头族 + attribution blob
//!
//! 出处：形态逐字段镜像 Magpie packages/zcode dress()（ZCode 3.17.2/3.19.1），
//! zcode_system.json 取自 ZCodeHub 实测 200 的 3.19.1 对齐版（官方客户端
//! prompt 数据，非 ZCodeHub 原创代码；出处标注见文件头部 JSON 键）。

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

pub const CHAT_APP_VERSION: &str = "3.19.1";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
pub const ANTHROPIC_SDK_UA: &str = "ai-sdk/anthropic/3.0.81";
pub const HTTP_REFERER: &str = "https://zcode.z.ai";
pub const MAX_TOKENS_LIMIT: i64 = 131072;

/// Plan 通道（JWT）：zcode.z.ai 的 Start/Coding Plan anthropic 端点
pub const PLAN_MESSAGES_URL: &str = "https://zcode.z.ai/api/v1/zcode-plan/anthropic/v1/messages";
/// Key 回退通道：api.z.ai 标准 anthropic 端点
pub const FALLBACK_MESSAGES_URL: &str = "https://api.z.ai/api/anthropic/v1/messages";
/// bigmodel Key 通道
pub const BIGMODEL_MESSAGES_URL: &str = "https://open.bigmodel.cn/api/anthropic/v1/messages";

/// 大小写别名 + 退役名改写（Z.AI 上游模型名大小写敏感；退役别名改到当前主力）
pub fn map_model_name(model: &str) -> String {
    match model.trim().to_lowercase().as_str() {
        "glm-5.4" => "GLM-5.4".into(),
        "glm-5.3-flash" | "glm-5.3-flash@chineseglm" => "GLM-5.3-Flash".into(),
        "glm-5.3" => "GLM-5.3".into(),
        "glm-5.2" | "glm-5-turbo" | "glm-turbo" | "glm-5.1" | "glm-4.7" => "GLM-5.3".into(),
        other => other.to_string(),
    }
}

/// /v1/models 对外公布（按当前套餐实测可用窗口）
pub const AVAILABLE_MODELS: &[&str] = &["GLM-5.4", "GLM-5.3-Flash", "GLM-5.3"];

/// 客户端透传头剔除（zcode2api agent._DROP_HEADERS 语义）：
/// 客户端自带的身份/追踪/代理头一律丢弃，上游只看到本网关仿真的身份。
const DROP_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "x-api-key",
    "authorization",
    "user-agent",
    "http-referer",
    "referer",
    "origin",
    "cookie",
    "accept",
    "accept-language",
    "accept-encoding",
    "connection",
    "true-client-ip",
    "x-original-forwarded-for",
    "content-type",
    "anthropic-version",
    "anthropic-beta",
    "x-device-mid",
    "x-request-id",
    "x-zcode-trace-id",
    "x-zcode-session-type",
    "x-query-id",
    "x-session-id",
    "x-title",
    "x-platform",
    "x-release-channel",
    "x-client-language",
    "x-client-timezone",
    "x-os-category",
    "x-os-version",
];
const DROP_PREFIXES: &[&str] = &[
    "x-zcode", "x-stainless", "x-forwarded", "forwarded", "x-real-ip", "via", "cf-", "cdn-loop",
];

pub fn is_dropped_header(name: &str) -> bool {
    let l = name.to_lowercase();
    DROP_HEADERS.contains(&l.as_str()) || DROP_PREFIXES.iter().any(|p| l.starts_with(p))
}

/// Plan 通道身份头族（identity.build_identity_headers 语义，linux 桌面实况）
/// `mid` 用账号级 virtual_device_mid（每号独立设备指纹），非全局值。
pub fn plan_identity_headers(mid: Option<&str>) -> Vec<(String, String)> {
    let tz = crate::quota::client_timezone();
    let mut h = vec![
        (
            "User-Agent".into(),
            format!("ZCode/{CHAT_APP_VERSION} {ANTHROPIC_SDK_UA}"),
        ),
        ("HTTP-Referer".into(), HTTP_REFERER.into()),
        ("X-Title".into(), "Z Code@electron".into()),
        ("X-ZCode-App-Version".into(), CHAT_APP_VERSION.into()),
        ("X-ZCode-Agent".into(), "glm".into()),
        ("X-Platform".into(), crate::quota::client_platform()),
        ("X-Release-Channel".into(), "production".into()),
        ("X-Client-Language".into(), "zh-CN".into()),
        ("X-Client-Timezone".into(), tz),
        ("X-Os-Category".into(), std::env::consts::OS.into()),
    ];
    if let Some(osv) = crate::quota::os_version() {
        h.push(("X-Os-Version".into(), osv));
    }
    if let Some(mid) = mid {
        h.push(("X-Device-Mid".into(), mid.to_string()));
    }
    h
}

/// 追踪头（identity.build_trace_headers 语义）：query_id 一次提问带出的全部
/// 请求共用（uuid5(轮数) 派生，确定性），session_id 会话内稳定。
pub fn trace_headers(session_id: &str, query_id: Option<&str>) -> Vec<(String, String)> {
    let mut h = vec![
        ("x-request-id".into(), uuid::Uuid::new_v4().to_string()),
        ("x-zcode-session-type".into(), "main".into()),
        ("x-zcode-trace-id".into(), uuid::Uuid::new_v4().to_string()),
        ("x-zcode-query-source".into(), "main_turn".into()),
        ("x-session-id".into(), session_id.to_string()),
    ];
    if let Some(q) = query_id {
        h.push(("x-query-id".into(), q.to_string()));
    }
    h
}

// ── body 变换 ───────────────────────────────────────────────────────────────

const PROMPT_JSON: &str = include_str!("zcode_system.json");

fn prompt() -> Option<Value> {
    serde_json::from_str(PROMPT_JSON).ok()
}

fn is_date_turn_text(text: &Value) -> bool {
    text.as_str()
        .is_some_and(|t| t.contains("<system-reminder>") && t.contains("Today's date is"))
}

/// 归一客户端 system 为 text block 列表
fn normalize_user_system(system: Option<&Value>) -> Vec<Value> {
    let Some(sys) = system else { return vec![] };
    if let Some(s) = sys.as_str() {
        if !s.trim().is_empty() {
            return vec![json!({ "type": "text", "text": s })];
        }
        return vec![];
    }
    let Some(arr) = sys.as_array() else { return vec![] };
    arr.iter()
        .filter_map(|item| {
            if let Some(s) = item.as_str() {
                (!s.trim().is_empty()).then(|| json!({ "type": "text", "text": s }))
            } else if item.get("type").and_then(Value::as_str) == Some("text")
                && item.get("text").and_then(Value::as_str).is_some_and(|t| !t.trim().is_empty())
            {
                Some(json!({ "type": "text", "text": item["text"] }))
            } else {
                None
            }
        })
        .collect()
}

/// environment 动态块（网关不掌握下游 cwd/git，按未知 place 形态省略，只留
/// platform / OS Version / untrustedNote；与 X-Platform / X-Os-Version 头同源）
fn environment_text(prompt: &Value) -> String {
    let e = prompt.get("environment").cloned().unwrap_or(Value::Null);
    let plat_arch = crate::quota::client_platform();
    let (plat, arch) = plat_arch.split_once('-').unwrap_or(("linux", "x64"));
    let osv = crate::quota::os_version().unwrap_or_else(|| "6.x".into());
    [
        e.get("heading").and_then(Value::as_str).unwrap_or("# Environment").to_string(),
        e.get("invokedLine").and_then(Value::as_str).unwrap_or("You have been invoked in the following environment: ").to_string(),
        format!(
            " - {}: {}",
            e.get("platformLabel").and_then(Value::as_str).unwrap_or("Platform"),
            plat
        ),
        format!(
            " - {}: {} {} {}",
            e.get("osVersionLabel").and_then(Value::as_str).unwrap_or("OS Version"),
            plat,
            osv,
            arch
        ),
        e.get("untrustedNote").and_then(Value::as_str).unwrap_or("").to_string(),
    ]
    .join("\n")
}

/// 前置官方 system 三块，客户端自身 system 并入末块（严格三块形态，多块即 3012）。
/// 幂等：首块已是 cliPrefix 则跳过。
pub fn apply_start_plan_system(body: &mut Value) -> bool {
    let Some(p) = prompt() else { return false };
    let Some(cli_prefix) = p.get("cliPrefix").and_then(Value::as_str) else { return false };
    let Some(stable) = p.get("stableSections").and_then(Value::as_array) else { return false };
    if stable.len() < 2 {
        return false;
    }
    // 幂等
    if let Some(first) = body.get("system").and_then(Value::as_array).and_then(|a| a.first()) {
        if first.get("text").and_then(Value::as_str) == Some(cli_prefix) {
            return false;
        }
    }

    let own: Vec<String> = normalize_user_system(body.get("system"))
        .iter()
        .filter_map(|b| {
            let t = b.get("text")?.as_str()?.trim().to_string();
            (!t.is_empty()).then_some(t)
        })
        .collect();
    let has_skill = body
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|ts| ts.iter().any(|t| t.get("name").and_then(Value::as_str) == Some("Skill")));
    let model = body.get("model").and_then(Value::as_str).unwrap_or("").trim().to_string();

    let mut dynamic: Vec<String> = vec![p.get("dynamicBehavior").and_then(Value::as_str).unwrap_or("").to_string()];
    if has_skill {
        if let Some(sg) = p.get("sessionGuidance").and_then(Value::as_str) {
            dynamic.push(sg.to_string());
        }
    }
    dynamic.push(environment_text(&p));
    if !model.is_empty() {
        let powered = p.get("poweredBy").and_then(Value::as_str).unwrap_or("");
        if !powered.is_empty() {
            dynamic.push(powered.replace("{provider}", "zai-api").replace("{model}", &model));
        }
    }
    dynamic.push(p.get("contextManagement").and_then(Value::as_str).unwrap_or("").to_string());
    dynamic.extend(own);

    let cached = |text: String| {
        json!({ "type": "text", "text": text, "cache_control": { "type": "ephemeral" } })
    };
    body["system"] = json!([
        cached(cli_prefix.to_string()),
        cached(stable.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n\n")),
        cached(format!("\n\n{}", dynamic.join("\n\n"))),
    ]);
    true
}

/// 官方缓存标记：清客户端全部 cache_control（非 system block + tools），只给
/// 最后一条非 system 消息的最后一个 block 打 ephemeral。
pub fn apply_cache_control(body: &mut Value) {
    let Some(msgs) = body.get_mut("messages").and_then(Value::as_array_mut) else { return };
    let mut last_idx: Option<usize> = None;
    for (i, msg) in msgs.iter_mut().enumerate() {
        if msg.get("role").and_then(Value::as_str) == Some("system") {
            continue;
        }
        last_idx = Some(i);
        if let Some(blocks) = msg.get_mut("content").and_then(Value::as_array_mut) {
            for block in blocks {
                if let Some(obj) = block.as_object_mut() {
                    obj.remove("cache_control");
                }
            }
        }
    }
    if let Some(i) = last_idx {
        let content = msgs[i].get_mut("content");
        match content {
            Some(Value::String(s)) if !s.is_empty() => {
                let text = s.clone();
                msgs[i]["content"] = json!([{ "type": "text", "text": text, "cache_control": { "type": "ephemeral" } }]);
            }
            Some(Value::Array(blocks)) if !blocks.is_empty() => {
                let n = blocks.len();
                if let Some(obj) = blocks[n - 1].as_object_mut() {
                    obj.insert("cache_control".into(), json!({ "type": "ephemeral" }));
                }
            }
            _ => {}
        }
    }
    if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) {
        for t in tools {
            if let Some(obj) = t.as_object_mut() {
                obj.remove("cache_control");
            }
        }
    }
}

/// 官方「当前日期」user 轮前插（幂等：已是该形态则跳过）
pub fn apply_current_date_turn(body: &mut Value) {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let date_text = format!("<system-reminder>Today's date is {today}.</system-reminder>");
    let already = body
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|m| m.first())
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|b| b.get("text"))
        .map(is_date_turn_text)
        .unwrap_or(false);
    if already {
        return;
    }
    let date_turn = json!({ "role": "user", "content": [{ "type": "text", "text": date_text }] });
    if let Some(msgs) = body.get_mut("messages").and_then(Value::as_array_mut) {
        let mut new_msgs = vec![date_turn];
        new_msgs.extend(msgs.drain(..));
        *msgs = new_msgs;
    }
}

/// attribution blob（官方 Bdt 形态）：`{"device_id":mid,"account_uuid":"","session_id":sid}`
/// 键序逐字节对齐官方（serde_json 默认按字母序重排 Map，因此手工拼装）。
pub fn attribution_user_id(device_mid: Option<&str>, session_id: Option<&str>) -> String {
    format!(
        "{{\"device_id\":{},\"account_uuid\":\"\",\"session_id\":{}}}",
        json_escape(device_mid.unwrap_or("")),
        json_escape(session_id.unwrap_or(""))
    )
}

fn json_escape(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

pub fn apply_user_id(body: &mut Value, user_id: &str) {
    let merged = match body.get("metadata") {
        Some(Value::Object(m)) => {
            let mut m = m.clone();
            m.insert("user_id".into(), Value::String(user_id.to_string()));
            m
        }
        _ => {
            let mut m = Map::new();
            m.insert("user_id".into(), Value::String(user_id.to_string()));
            m
        }
    };
    body["metadata"] = Value::Object(merged);
}

/// 按字符数截断（Unicode 安全，用于日志预览）
pub fn truncate_chars(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let cut: String = s.chars().take(n).collect();
    format!("{cut}…")
}

/// 会话稳定 session_id：uuid5(sha256(会话首条非日期 user 文本)[:32])，带 account
/// 命名空间隔离。必须跳过注入的日期轮，否则重试/换号二次变换后指纹漂移。
pub fn conversation_session_id(body: &Value, account_id: &str) -> Option<String> {
    let text = extract_first_user_text(body)?;
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hex_short(&hasher.finalize(), 16);
    Some(uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        format!("zcode-switch:{account_id}:{digest}").as_bytes(),
    ).to_string())
}

fn hex_short(bytes: &[u8], hex_len: usize) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
        [..hex_len]
        .to_string()
}

/// 提取会话首条非日期注入的用户文本
pub fn extract_first_user_text(body: &Value) -> Option<String> {
    let msgs = body.get("messages").and_then(Value::as_array)?;
    for msg in msgs {
        if msg.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        match msg.get("content") {
            Some(Value::String(s)) => {
                let t = s.trim();
                if !t.is_empty() && !is_date_turn_text(&Value::String(t.to_string())) {
                    return Some(t.to_string());
                }
            }
            Some(Value::Array(blocks)) => {
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) == Some("text") {
                        if let Some(t) = block.get("text").and_then(Value::as_str) {
                            let tt = t.trim();
                            if !tt.is_empty() && !is_date_turn_text(&Value::String(t.to_string())) {
                                return Some(tt.to_string());
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// 轮次 x-query-id：uuid5(session_id, 带提问语义的 user 轮数)。无提问轮 → None
/// （官方对无可归因请求不发该头）。
pub fn conversation_query_id(body: &Value, session_id: Option<&str>) -> Option<String> {
    let msgs = body.get("messages").and_then(Value::as_array)?;
    let mut turns = 0usize;
    for msg in msgs {
        if msg.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        if message_reads_as_query(msg) {
            turns += 1;
        }
    }
    if turns == 0 {
        return None;
    }
    Some(uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        format!("zcode-query\0{}\0{turns}", session_id.unwrap_or("")).as_bytes(),
    ).to_string())
}

fn message_reads_as_query(msg: &Value) -> bool {
    match msg.get("content") {
        Some(Value::String(s)) => {
            !s.trim().is_empty() && !is_date_turn_text(&Value::String(s.clone()))
        }
        Some(Value::Array(blocks)) => {
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("tool_result") => continue,
                    Some("text") => {
                        let t = block.get("text").and_then(Value::as_str).unwrap_or("");
                        if !t.trim().is_empty() && !is_date_turn_text(&Value::String(t.to_string())) {
                            return true;
                        }
                    }
                    _ => return true, // image 等其它块同算一次提问
                }
            }
            false
        }
        _ => false,
    }
}

/// 请求体归一：模型名映射、max_tokens 钳制、字符串 content 转 block。
/// 畸形输入保持原样（变换永不放大请求失败）。
pub fn normalize_body(body: &mut Value) {
    if let Some(m) = body.get("model").and_then(Value::as_str) {
        body["model"] = Value::String(map_model_name(m));
    }
    if let Some(mt) = body.get("max_tokens") {
        let clamped = mt.as_i64().unwrap_or(4096).clamp(1, MAX_TOKENS_LIMIT);
        body["max_tokens"] = Value::from(clamped);
    } else {
        body["max_tokens"] = Value::from(4096);
    }
    if let Some(msgs) = body.get_mut("messages").and_then(Value::as_array_mut) {
        for msg in msgs {
            if let Some(Value::String(s)) = msg.get("content") {
                if !s.is_empty() {
                    let text = s.clone();
                    msg["content"] = json!([{ "type": "text", "text": text }]);
                }
            }
        }
    }
}

/// Plan 通道全量变换（transform_body 语义）。变换失败静默保持原样并留痕。
pub fn transform_plan_body(body: &mut Value, device_mid: Option<&str>, session_id: Option<&str>) {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        apply_start_plan_system(body);
        apply_cache_control(body);
        apply_current_date_turn(body);
        if device_mid.is_some() || session_id.is_some() {
            apply_user_id(body, &attribution_user_id(device_mid, session_id));
        }
    }));
    if r.is_err() {
        eprintln!("[zsw-gw] body 变换异常（按原样发出，3012 风险）");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn system_blocks_strict_three() {
        let mut body = json!({ "model": "GLM-5.3", "messages": [{ "role": "user", "content": "hi" }] });
        assert!(apply_start_plan_system(&mut body));
        let sys = body["system"].as_array().unwrap();
        assert_eq!(sys.len(), 3, "严格三块");
        assert_eq!(sys[0]["text"], "You are ZCode, an interactive agent");
        assert_eq!(sys[0]["cache_control"]["type"], "ephemeral");
        // 幂等
        assert!(!apply_start_plan_system(&mut body));
        assert_eq!(body["system"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn user_system_merged_into_last_block() {
        let mut body = json!({
            "model": "GLM-5.3",
            "system": "You are a pirate.",
            "messages": [{ "role": "user", "content": "hi" }]
        });
        apply_start_plan_system(&mut body);
        let sys = body["system"].as_array().unwrap();
        assert_eq!(sys.len(), 3);
        let last = sys[2]["text"].as_str().unwrap();
        assert!(last.contains("You are a pirate."), "客户端 system 并入末块");
        assert!(last.contains("# Environment"));
    }

    #[test]
    fn date_turn_and_cache_control() {
        let mut body = json!({
            "model": "GLM-5.3",
            "messages": [
                { "role": "user", "content": "q1" },
                { "role": "assistant", "content": [{ "type": "text", "text": "a" }] },
                { "role": "user", "content": "q2" }
            ]
        });
        apply_current_date_turn(&mut body);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 4);
        let first_text = msgs[0]["content"][0]["text"].as_str().unwrap();
        assert!(first_text.contains("Today's date is"));
        assert!(first_text.contains("<system-reminder>"));
        // 幂等
        apply_current_date_turn(&mut body);
        assert_eq!(body["messages"].as_array().unwrap().len(), 4);

        apply_cache_control(&mut body);
        let last = body["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(last["content"][0]["cache_control"]["type"], "ephemeral");
        // 日期轮与中间消息无 cache_control
        let mid = &body["messages"].as_array().unwrap()[1];
        assert!(mid.get("cache_control").is_none());
    }

    #[test]
    fn attribution_blob_compact_shape() {
        let blob = attribution_user_id(Some("66d70bbf-1234"), Some("sess-1"));
        assert_eq!(blob, r#"{"device_id":"66d70bbf-1234","account_uuid":"","session_id":"sess-1"}"#);
    }

    #[test]
    fn session_id_stable_and_namespace_isolated() {
        let body = json!({ "messages": [
            { "role": "user", "content": "first question" },
            { "role": "assistant", "content": "ok" },
            { "role": "user", "content": "second" }
        ]});
        let s1 = conversation_session_id(&body, "acc1").unwrap();
        let s2 = conversation_session_id(&body, "acc1").unwrap();
        assert_eq!(s1, s2, "同会话同 id");
        let s3 = conversation_session_id(&body, "acc2").unwrap();
        assert_ne!(s1, s3, "跨账号命名空间隔离");
        // 日期注入轮不影响指纹
        let mut b2 = body.clone();
        apply_current_date_turn(&mut b2);
        assert_eq!(conversation_session_id(&b2, "acc1").unwrap(), s1);
    }

    #[test]
    fn query_id_counts_real_turns() {
        let body = json!({ "messages": [
            { "role": "user", "content": "q1" },
            { "role": "assistant", "content": "a" },
            { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "r" }] },
            { "role": "user", "content": "q2" }
        ]});
        let sid = conversation_session_id(&body, "a").unwrap();
        let q = conversation_query_id(&body, Some(&sid)).unwrap();
        let q2 = conversation_query_id(&body, Some(&sid)).unwrap();
        assert_eq!(q, q2, "确定性");
        // 只有 tool_result 的轮不算提问
        let only_tool = json!({ "messages": [
            { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "r" }] }
        ]});
        assert!(conversation_query_id(&only_tool, Some(&sid)).is_none());
    }

    #[test]
    fn model_map_and_clamp() {
        assert_eq!(map_model_name("glm-5.4"), "GLM-5.4");
        assert_eq!(map_model_name("glm-5.3-flash"), "GLM-5.3-Flash");
        assert_eq!(map_model_name("GLM-5.2"), "GLM-5.3", "退役别名改写");
        assert_eq!(map_model_name("GLM-5.3"), "GLM-5.3");
        let mut body = json!({ "model": "glm-5.2", "max_tokens": 999999, "messages": [{ "role": "user", "content": "plain" }] });
        normalize_body(&mut body);
        assert_eq!(body["model"], "GLM-5.3");
        assert_eq!(body["max_tokens"], MAX_TOKENS_LIMIT);
        assert!(body["messages"][0]["content"].is_array(), "字符串 content 转 block");
    }

    #[test]
    fn dropped_headers_cover_client_identity() {
        for h in ["x-device-mid", "X-Session-Id", "X-Stainless-Retry", "cf-connecting-ip", "Authorization", "anthropic-beta"] {
            assert!(is_dropped_header(h), "{h} 应被剔除");
        }
        assert!(!is_dropped_header("x-custom-business"));
    }
}
