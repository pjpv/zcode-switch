//! 会话亲和：把同一会话的请求粘到同一账号，最大化上游 prompt cache 命中。
//!
//! key 提取优先级（zcode2api affinity.extract_context_key 语义）：
//!   1. 显式会话头（x-session-id 等）→ "{provider}:hdr:{id}"
//!   2. body metadata.session_id / 根级 session_id → "{provider}:meta:{id}"
//!   3. 首条 user 文本 sha256[:32] + 客户端 IP → "{provider}:conv:{ip}:{digest}"
//!
//! 绑定时机：仅成功建立上游流后写入（4xx 不绑定）；延续判定：显式 key 时
//! 有既有绑定即延续，指纹模式必须有 assistant 历史轮（纯单轮按新请求均匀分流）。
//! TTL 3600s 滑动续期，上限 10000 条（超限先清过期再淘汰最老 20%）。

use std::collections::HashMap;
use std::sync::Mutex;

const DEFAULT_TTL_SECS: u64 = 3600;
const DEFAULT_MAX_ENTRIES: usize = 10000;

const SESSION_HEADERS: &[&str] = &[
    "x-session-id",
    "session-id",
    "x-conversation-id",
    "conversation-id",
    "x-chat-id",
    "chat-id",
];

pub struct Affinity {
    cache: Mutex<HashMap<String, (String, i64)>>, // key -> (account_id, expire_epoch_s)
    ttl_secs: u64,
    max_entries: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextInfo {
    pub key: String,
    /// true = 判定为同一会话的延续，允许亲和钉号
    pub is_continuation: bool,
}

impl Affinity {
    pub fn new() -> Affinity {
        Affinity {
            cache: Mutex::new(HashMap::new()),
            ttl_secs: DEFAULT_TTL_SECS,
            max_entries: DEFAULT_MAX_ENTRIES,
        }
    }

    pub fn header_value<'a>(&self, headers: &'a [(String, String)]) -> Option<&'a str> {
        SESSION_HEADERS.iter().find_map(|name| {
            headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
                .filter(|v| !v.trim().is_empty())
        })
    }

    /// 提取会话上下文（provider 前缀隔离命名空间）
    pub fn extract(
        &self,
        provider: &str,
        headers: &[(String, String)],
        client_ip: &str,
        body: &serde_json::Value,
    ) -> ContextInfo {
        let now = now_s();
        // 1) 显式会话头
        if let Some(id) = self.header_value(headers) {
            let key = format!("{provider}:hdr:{}", id.trim());
            let pinned = self.get(&key, now);
            let continuation = has_assistant_history(body) || pinned.is_some();
            return ContextInfo { key, is_continuation: continuation };
        }
        // 2) body metadata.session_id / 根级 session_id
        let meta_sid = body
            .pointer("/metadata/session_id")
            .and_then(serde_json::Value::as_str)
            .or_else(|| body.get("session_id").and_then(serde_json::Value::as_str))
            .filter(|s| !s.trim().is_empty());
        if let Some(id) = meta_sid {
            let key = format!("{provider}:meta:{}", id.trim());
            let pinned = self.get(&key, now);
            let continuation = has_assistant_history(body) || pinned.is_some();
            return ContextInfo { key, is_continuation: continuation };
        }
        // 3) 首条 user 文本指纹 + IP：必须有 assistant 历史轮才视为延续
        let digest = super::transform::extract_first_user_text(body)
            .map(|text| {
                use sha2::{Digest, Sha256};
                let mut h = Sha256::new();
                h.update(text.as_bytes());
                h.finalize()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
                    [..32]
                    .to_string()
            })
            .unwrap_or_default();
        let key = format!("{provider}:conv:{}:{}", client_ip, digest);
        let continuation = has_assistant_history(body);
        ContextInfo { key, is_continuation: continuation }
    }

    fn get(&self, key: &str, now: i64) -> Option<String> {
        let Ok(mut g) = self.cache.lock() else { return None };
        g.retain(|_, (_, exp)| *exp > now);
        g.get_mut(key).map(|(id, exp)| {
            let id = id.clone();
            *exp = now + self.ttl_secs as i64; // 滑动续期
            id
        })
    }

    pub fn pinned_account(&self, ctx: &ContextInfo, now: i64) -> Option<String> {
        if !ctx.is_continuation {
            return None;
        }
        self.get(&ctx.key, now)
    }

    /// 仅成功建流后调用
    pub fn bind(&self, ctx: &ContextInfo, account_id: &str) {
        let Ok(mut g) = self.cache.lock() else { return };
        let now = now_s();
        g.retain(|_, (_, exp)| *exp > now);
        if g.len() >= self.max_entries {
            // 淘汰最老 20%
            let mut by_exp: Vec<(String, i64)> =
                g.iter().map(|(k, (_, e))| (k.clone(), *e)).collect();
            by_exp.sort_by_key(|(_, e)| *e);
            let cutoff = by_exp.len() / 5;
            for (k, _) in by_exp.into_iter().take(cutoff.max(1)) {
                g.remove(&k);
            }
        }
        g.insert(ctx.key.clone(), (account_id.to_string(), now + self.ttl_secs as i64));
    }

    pub fn len(&self) -> usize {
        self.cache.lock().map(|g| g.len()).unwrap_or(0)
    }
}

impl Default for Affinity {
    fn default() -> Self {
        Self::new()
    }
}

fn now_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn has_assistant_history(body: &serde_json::Value) -> bool {
    body.get("messages")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|msgs| {
            msgs.iter().any(|m| m.get("role").and_then(serde_json::Value::as_str) == Some("assistant"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hdrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn explicit_header_key_and_binding() {
        let a = Affinity::new();
        let h = hdrs(&[("x-session-id", "s1")]);
        let body = json!({ "messages": [{ "role": "user", "content": "hi" }] });
        let ctx = a.extract("zai", &h, "127.0.0.1", &body);
        assert_eq!(ctx.key, "zai:hdr:s1");
        assert!(!ctx.is_continuation, "单轮无绑定不钉号");
        a.bind(&ctx, "acc1");
        let ctx2 = a.extract("zai", &h, "127.0.0.1", &body);
        assert!(ctx2.is_continuation, "有绑定即延续");
        assert_eq!(a.pinned_account(&ctx2, now_s()), Some("acc1".into()));
    }

    #[test]
    fn metadata_session_id_fallback() {
        let a = Affinity::new();
        let body = json!({ "metadata": { "session_id": "m1" }, "messages": [{ "role": "user", "content": "x" }] });
        let ctx = a.extract("zai", &hdrs(&[]), "10.0.0.1", &body);
        assert_eq!(ctx.key, "zai:meta:m1");
    }

    #[test]
    fn fingerprint_requires_assistant_history() {
        let a = Affinity::new();
        let single = json!({ "messages": [{ "role": "user", "content": "only turn" }] });
        let ctx = a.extract("zai", &hdrs(&[]), "10.0.0.2", &single);
        assert!(!ctx.is_continuation, "指纹模式纯单轮不延续");
        let multi = json!({ "messages": [
            { "role": "user", "content": "first" },
            { "role": "assistant", "content": "ok" },
            { "role": "user", "content": "second" }
        ]});
        let ctx2 = a.extract("zai", &hdrs(&[]), "10.0.0.2", &multi);
        assert!(ctx2.is_continuation);
        a.bind(&ctx2, "acc9");
        assert_eq!(a.pinned_account(&ctx2, now_s()), Some("acc9".into()));
    }

    #[test]
    fn provider_namespace_isolation() {
        let a = Affinity::new();
        let h = hdrs(&[("x-session-id", "s9")]);
        let body = json!({ "messages": [{ "role": "user", "content": "x" }] });
        let c1 = a.extract("zai", &h, "ip", &body);
        let c2 = a.extract("bigmodel", &h, "ip", &body);
        assert_ne!(c1.key, c2.key);
    }
}
