//! 上游被拒信号 → 账号动作。
//!
//! 判定纪律（移植自 ZCodeHub，含踩坑注释）：业务码一律**精确解析** JSON body
//! 的 `code` 字段，绝不裸子串匹配 —— 响应体里的 logid 是 20 位随机数字，
//! 裸子串 "3009"/"1113" 会撞车；"quota" 裸词会把风控页判成额度耗尽。

use serde_json::Value;

/// 业务码集合（出处见 zcode2api app/constants.py 252-307，逐条实测依据保留在注释）
pub const NO_BALANCE_CODE: i64 = 1113;
pub const EXHAUST_CODES: &[i64] = &[1005, 1113, 1310];
pub const CAPTCHA_CODE: i64 = 3007;
pub const RISK_CODE: i64 = 3012;
pub const CONCURRENCY_CODES: &[i64] = &[3008, 3009, 3010];
pub const RATE_LIMIT_429_CODES: &[i64] = &[1302, 1303];

/// 定量无额度短语（完整词 + 明确语义；不用裸 "quota"）
const EXHAUST_KEYWORDS: &[&str] = &[
    "exceed quota",
    "quota limit",
    "quota exceeded",
    "quota exhausted",
    "insufficient balance",
    "insufficient quota",
    "no balance",
    "额度已用完",
    "额度用完",
    "额度不足",
    "余额不足",
    "余额已用完",
    "无余额",
    "使用上限",
    "weekly limit",
    "monthly limit",
    "rate limit exceeded",
];

/// 风控关键词（3012 unusual activity）
const RISK_MARKERS: &[&str] = &["unusual activity"];

/// 边缘封禁（IP 级，405 + HTML 页面文案；不惩罚账号，进程级暂停 plan 渠道）
pub const EDGE_BLOCK_MARKERS: &[&str] = &[
    "address was blocked",
    "network's address",
    "blocked by the edge",
];

/// 429 策略常量（秒）
pub const RETRY_429_TIMES: u32 = 5;
pub const RETRY_429_WAIT_MAX: u64 = 120;
pub const CONCURRENCY_WAIT_BASE: u64 = 3;
pub const CONCURRENCY_WAIT_MAX: u64 = 30;
pub const CONCURRENCY_GIVEUP_AFTER: u32 = 2;
pub const RATE_LIMIT_COOLDOWN: i64 = 60;
pub const RETRY_5XX_TIMES: u32 = 3;
pub const RETRY_5XX_WAIT: u64 = 5;
pub const COOLING_5XX: i64 = 300;
pub const CONNECT_FAIL_COOLDOWN: i64 = 60;
/// 风控指数退避：min(900 * 2^(n-1), 86400)，4 次 → DISABLED，7 天衰减
pub const RISK_COOLDOWN_BASE: i64 = 900;
pub const RISK_COOLDOWN_MAX: i64 = 86400;
pub const RISK_BAN_STRIKES: u32 = 4;
pub const RISK_STRIKE_DECAY_MS: i64 = 7 * 24 * 3600 * 1000;
/// 验证码挑战：MVP 无头求解器缺失，同号短冷却后换号
pub const CAPTCHA_COOLDOWN: i64 = 300;

pub fn business_code(body: &Value) -> Option<i64> {
    body.get("code").and_then(|c| c.as_i64())
}

pub fn body_msg(body: &Value) -> String {
    ["msg", "message", "error", "detail"]
        .iter()
        .find_map(|k| body.get(*k).and_then(|v| v.as_str()).map(String::from))
        .unwrap_or_default()
}

fn has_keyword(text: &str, kws: &[&str]) -> bool {
    let lower = text.to_lowercase();
    kws.iter().any(|k| lower.contains(k))
}

/// 单次上游响应的判定结果 → 调度器的处置动作
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// 2xx：流式/非流式均按成功透传
    Ok,
    /// Plan 通道额度耗尽（402 / 1005 / 1113 / 1310 / 定量短语）
    Exhausted,
    /// 真限流（1302/1303）：账号冷却 RATE_LIMIT_COOLDOWN 后换号
    RateLimited,
    /// 并发争用（3008/3009/3010）：不冷却账号，短退避原地重试
    Concurrency,
    /// 其它 429：不冷却账号，按 Retry-After（封顶 120s）原地重试
    Rate429 { retry_after: Option<u64> },
    /// 风控（3012 / 405 非边缘页）：账号指数退避
    Risk,
    /// 边缘封 IP（405 + 封禁页文案）：账号不惩罚
    EdgeBlock,
    /// 验证码挑战（3007 / 403+captcha 文案 / 挑战响应头）
    Captcha,
    /// 凭据失效（401/403 非挑战）
    Auth,
    /// 5xx：重试 3×5s 后冷却 300s 换号
    ServerErr { status: u16 },
    /// 连接层失败（超时/DNS/断连）：60s 短冷却（出口瞬态，非账号故障）
    ConnectFail,
    /// 其它 4xx：原样回传客户端，fail_count +1
    ClientError { status: u16 },
}

/// 从上游响应分类。`status` 为 HTTP 状态码；400-499 时尝试解析 body 业务码。
pub fn classify(status: u16, body: Option<&Value>, headers: &[(String, String)]) -> Verdict {
    let hdr = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    let msg = body.map(body_msg).unwrap_or_default();
    let body_text = body.map(|b| b.to_string()).unwrap_or_default();

    // 1) 验证码挑战：挑战响应头 / body 3007 / 403+captcha 文案
    let captcha_hdr = hdr("X-Aliyun-Captcha-Verify-Param").is_some()
        || hdr("X-Aliyun-Captcha-Verify-Region").is_some();
    if captcha_hdr
        || body.and_then(business_code) == Some(CAPTCHA_CODE)
        || (status == 403 && (msg.to_lowercase().contains("captcha") || msg.contains("验证码")))
    {
        return Verdict::Captcha;
    }

    // 2) 业务码精确解析（HTTP 200 承载业务错误时同样生效）
    let code = body.and_then(business_code);
    if let Some(c) = code {
        if EXHAUST_CODES.contains(&c) {
            return Verdict::Exhausted;
        }
        if CONCURRENCY_CODES.contains(&c) {
            return Verdict::Concurrency;
        }
        if RATE_LIMIT_429_CODES.contains(&c) {
            return Verdict::RateLimited;
        }
        if c == RISK_CODE {
            return Verdict::Risk;
        }
        if c == 401 {
            return Verdict::Auth;
        }
    }

    // 3) 定量无额度短语（仅 2xx 承载业务错误或 4xx 文案时）
    if !msg.is_empty() && has_keyword(&msg, EXHAUST_KEYWORDS) && !has_keyword(&msg, RISK_MARKERS) {
        return Verdict::Exhausted;
    }

    match status {
        200..=299 => Verdict::Ok,
        401 | 403 => Verdict::Auth,
        402 => Verdict::Exhausted,
        405 => {
            if has_keyword(&body_text, EDGE_BLOCK_MARKERS) {
                Verdict::EdgeBlock
            } else {
                Verdict::Risk
            }
        }
        429 => {
            let ra = hdr("retry-after")
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|s| *s > 0);
            Verdict::Rate429 { retry_after: ra.map(|s| s.min(RETRY_429_WAIT_MAX)) }
        }
        500..=599 => Verdict::ServerErr { status },
        400..=499 => Verdict::ClientError { status },
        _ => Verdict::ServerErr { status },
    }
}

/// 风控指数退避秒数：min(base * 2^(n-1), cap)
pub fn risk_cooldown_secs(strikes: u32) -> i64 {
    let mut secs = RISK_COOLDOWN_BASE;
    for _ in 1..strikes {
        secs = (secs.saturating_mul(2)).min(RISK_COOLDOWN_MAX);
    }
    secs.min(RISK_COOLDOWN_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn no_headers() -> Vec<(String, String)> {
        vec![]
    }

    #[test]
    fn parses_business_codes_precisely() {
        // 精确码：200 body 承载业务错误
        assert_eq!(classify(200, Some(&json!({"code": 3009, "msg": "model concurrency limit exceeded"})), &no_headers()), Verdict::Concurrency);
        assert_eq!(classify(200, Some(&json!({"code": 1005, "msg": "exceed quota limit"})), &no_headers()), Verdict::Exhausted);
        assert_eq!(classify(200, Some(&json!({"code": 1302})), &no_headers()), Verdict::RateLimited);
        assert_eq!(classify(405, Some(&json!({"code": 3012, "msg": "This client is not supported."})), &no_headers()), Verdict::Risk);
        assert_eq!(classify(400, Some(&json!({"code": 3007})), &no_headers()), Verdict::Captcha);
    }

    #[test]
    fn no_substring_false_positives() {
        // 20 位 logid 不允许撞出业务码（裸子串陷阱回归测试）
        let body = json!({"code": 0, "msg": "ok", "logid": "30091234567890123456"});
        assert_eq!(classify(200, Some(&body), &no_headers()), Verdict::Ok);
        let quota_word = json!({"code": 0, "msg": "see quota docs"});
        assert_eq!(classify(200, Some(&quota_word), &no_headers()), Verdict::Ok);
    }

    #[test]
    fn edge_block_vs_risk_on_405() {
        let edge = json!({"detail": "Your network's address was blocked by the edge."});
        assert_eq!(classify(405, Some(&edge), &no_headers()), Verdict::EdgeBlock);
        assert_eq!(classify(405, Some(&json!({"detail": "method not allowed"})), &no_headers()), Verdict::Risk);
    }

    #[test]
    fn retry_after_parsed_and_capped() {
        let v = classify(429, Some(&json!({"msg": "slow down"})), &[("Retry-After".into(), "999".into())]);
        match v {
            Verdict::Rate429 { retry_after } => assert_eq!(retry_after, Some(RETRY_429_WAIT_MAX)),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            classify(429, None, &no_headers()),
            Verdict::Rate429 { retry_after: None }
        ));
    }

    #[test]
    fn captcha_by_header_or_403_text() {
        let h = vec![("X-Aliyun-Captcha-Verify-Param".into(), "x".into())];
        assert_eq!(classify(200, Some(&json!({"code": 0})), &h), Verdict::Captcha);
        let f403 = json!({"detail": "captcha required"});
        assert_eq!(classify(403, Some(&f403), &no_headers()), Verdict::Captcha);
    }

    #[test]
    fn risk_backoff_is_exponential_with_cap() {
        assert_eq!(risk_cooldown_secs(1), 900);
        assert_eq!(risk_cooldown_secs(2), 1800);
        assert_eq!(risk_cooldown_secs(3), 3600);
        assert_eq!(risk_cooldown_secs(8), RISK_COOLDOWN_MAX);
    }
}
