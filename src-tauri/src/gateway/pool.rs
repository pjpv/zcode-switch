//! 网关账号池：每个库内账号的运行态（状态机/冷却/风控/额度快照）与选号算法。
//!
//! 选号比较器移植自 ZCodeHub 的成对比较语义（见仓库内注释出处）：
//! 在飞数升序 → 同负载下额度剩余显著更富者优先（5% 相对容差，带内交给轮询）→
//! 轮询游标均摊。冷却/耗尽/禁用是过滤条件，不参与排序。
//!
//! 运行态持久化在 `~/.zcode-switch/gateway-state.json`，重启后冷却/风控计数
//! 不丢；在飞计数是纯内存（重启清零，短暂可能超上游单号并发，可接受）。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// 账号状态机（语义对齐 ZCodeHub：INVALID/DISABLED 永不被成功响应洗回 ACTIVE）
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PoolStatus {
    Active,
    Exhausted,
    Cooling,
    Invalid,
    Disabled,
}

impl PoolStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            PoolStatus::Active => "active",
            PoolStatus::Exhausted => "exhausted",
            PoolStatus::Cooling => "cooling",
            PoolStatus::Invalid => "invalid",
            PoolStatus::Disabled => "disabled",
        }
    }
}

/// 单账号运行态
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct AccountState {
    pub status: Option<PoolStatus>,
    /// 冷却截止（epoch ms）；0 = 未冷却
    #[serde(default)]
    pub cooling_until: i64,
    /// 风控连击数（3012/405），指数退避基数见 error.rs
    #[serde(default)]
    pub risk_strikes: u32,
    #[serde(default)]
    pub last_risk_at: i64,
    #[serde(default)]
    pub use_count: u64,
    #[serde(default)]
    pub fail_count: u64,
    #[serde(default)]
    pub last_used_at: i64,
    #[serde(default)]
    pub last_error: String,
    /// 最近一次额度快照（quota.rs 的 QuotaOverview 原样缓存）
    #[serde(default)]
    pub quota: Option<Value>,
    #[serde(default)]
    pub quota_at: i64,
}

impl AccountState {
    pub fn status_or_active(&self) -> PoolStatus {
        self.status.unwrap_or(PoolStatus::Active)
    }

    fn effective_status(&self, now_ms: i64) -> PoolStatus {
        let st = self.status_or_active();
        // 冷却惰性恢复：到期即视为 active（状态字段下次落盘时校正）
        if st == PoolStatus::Cooling && self.cooling_until <= now_ms {
            return PoolStatus::Active;
        }
        st
    }

    pub fn is_selectable(&self, now_ms: i64) -> bool {
        matches!(
            self.effective_status(now_ms),
            PoolStatus::Active | PoolStatus::Cooling
        )
    }

    /// 额度剩余：取 QuotaOverview.remaining，缺失回退 total-used；再缺失视为 0（未知劣于已知正余额）
    pub fn remaining(&self) -> f64 {
        let Some(q) = &self.quota else { return 0.0 };
        let rem = q.get("remaining").and_then(|v| v.as_f64());
        if let Some(r) = rem {
            return r.max(0.0);
        }
        match (
            q.get("total").and_then(|v| v.as_f64()),
            q.get("used").and_then(|v| v.as_f64()),
        ) {
            (Some(t), Some(u)) => (t - u).max(0.0),
            _ => 0.0,
        }
    }
}

pub const ACCOUNT_CONCURRENCY: usize = 3;
/// 单请求最多尝试的账号数（满载跳过不计入）
pub const MAX_ACCOUNT_ATTEMPTS: usize = 5;
/// 同负载下“显著更富”的相对容差
const SELECT_TIE_TOLERANCE: f64 = 0.05;

pub struct Pool {
    states: Mutex<HashMap<String, AccountState>>,
    inflight: Mutex<HashMap<String, usize>>,
    cursor: AtomicUsize,
    state_file: PathBuf,
}

impl Pool {
    pub fn new(state_file: PathBuf) -> Pool {
        let p = Pool {
            states: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
            cursor: AtomicUsize::new(0),
            state_file,
        };
        p.load_disk();
        p
    }

    fn load_disk(&self) {
        let Ok(raw) = std::fs::read_to_string(&self.state_file) else { return };
        let Ok(map) = serde_json::from_str::<HashMap<String, AccountState>>(&raw) else { return };
        if let Ok(mut g) = self.states.lock() {
            *g = map;
        }
    }

    pub fn persist(&self) {
        let Ok(g) = self.states.lock() else { return };
        let Ok(body) = serde_json::to_string_pretty(&*g) else { return };
        let tmp = self.state_file.with_extension("json.tmp");
        if std::fs::write(&tmp, body).is_ok() {
            let _ = std::fs::rename(&tmp, &self.state_file);
        }
    }

    pub fn snapshot_of(&self, id: &str) -> Option<AccountState> {
        self.states.lock().ok()?.get(id).cloned()
    }

    pub fn with_state<R>(&self, id: &str, f: impl FnOnce(&mut AccountState) -> R) -> Option<R> {
        let mut g = self.states.lock().ok()?;
        let st = g.entry(id.to_string()).or_default();
        Some(f(st))
    }

    pub fn inflight_of(&self, id: &str) -> usize {
        self.inflight.lock().map(|g| *g.get(id).unwrap_or(&0)).unwrap_or(0)
    }

    /// 单账号并发槽：检查+自增必须同步完成，避免 check-then-act 竞态。
    /// 满载返回 false（调用方跳过该号且不计入尝试预算）。
    pub fn acquire_slot(&self, id: &str) -> bool {
        let Ok(mut g) = self.inflight.lock() else { return false };
        let cur = g.entry(id.to_string()).or_insert(0);
        if *cur >= ACCOUNT_CONCURRENCY {
            return false;
        }
        *cur += 1;
        true
    }

    pub fn release_slot(&self, id: &str) {
        if let Ok(mut g) = self.inflight.lock() {
            if let Some(v) = g.get_mut(id) {
                *v = v.saturating_sub(1);
                if *v == 0 {
                    g.remove(id);
                }
            }
        }
    }

    /// 等待后重新占槽；占不到返回 false（调用方换号）。
    pub fn reacquire_slot(&self, id: &str) -> bool {
        self.acquire_slot(id)
    }

    /// 选号：亲和号优先 → 过滤（可选、不在 skip、槽位未满）→
    /// 在飞升序 → 5% 容差内剩余视为同档 → 轮询偏移。
    pub fn select(
        &self,
        ids: &[String],
        skip: &std::collections::HashSet<String>,
        pinned: Option<&str>,
    ) -> Option<String> {
        let now_ms = now_ms();
        if let Some(p) = pinned {
            if !skip.contains(p)
                && self.snapshot_of(p).is_some_and(|s| s.is_selectable(now_ms))
                && self.inflight_of(p) < ACCOUNT_CONCURRENCY
                && ids.iter().any(|i| i == p)
            {
                return Some(p.to_string());
            }
        }
        let mut candidates: Vec<&String> = ids
            .iter()
            .filter(|id| {
                !skip.contains(*id)
                    && self.snapshot_of(id).is_some_and(|s| s.is_selectable(now_ms))
                    && self.inflight_of(id) < ACCOUNT_CONCURRENCY
            })
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let n = candidates.len();
        let cur = self.cursor.load(Ordering::Relaxed);
        // 主排序：在飞升序；次排序：5% 容差内剩余视为同档，显著更富者优先
        candidates.sort_by(|a, b| {
            let sa = self.snapshot_of(a).unwrap_or_default();
            let sb = self.snapshot_of(b).unwrap_or_default();
            let ia = self.inflight_of(a);
            let ib = self.inflight_of(b);
            if ia != ib {
                return ia.cmp(&ib);
            }
            let ra = sa.remaining();
            let rb = sb.remaining();
            if rb > ra * (1.0 + SELECT_TIE_TOLERANCE) {
                std::cmp::Ordering::Greater // b 更富 → a 靠后
            } else if ra > rb * (1.0 + SELECT_TIE_TOLERANCE) {
                std::cmp::Ordering::Less
            } else {
                // 带内：轮询偏移均摊
                let pa = ids.iter().position(|i| i == *a).unwrap_or(0);
                let pb = ids.iter().position(|i| i == *b).unwrap_or(0);
                ((pa - cur).rem_euclid(n)).cmp(&((pb - cur).rem_euclid(n)))
            }
        });
        let best = candidates[0].clone();
        let bi = ids.iter().position(|i| *i == best).unwrap_or(0);
        self.cursor.store(bi + 1, Ordering::Relaxed);
        Some(best)
    }

    /// UI/CLI 的池视图（从未调度过的账号按默认状态渲染，不丢行）
    pub fn view(&self, ids: &[String], names: &HashMap<String, String>) -> Vec<Value> {
        let now_ms = now_ms();
        ids.iter()
            .filter_map(|id| {
                let st = self.snapshot_of(id).unwrap_or_default();
                let st_eff = st.effective_status(now_ms);
                Some(serde_json::json!({
                    "id": id,
                    "name": names.get(id).cloned().unwrap_or_else(|| id.clone()),
                    "status": st_eff.as_str(),
                    "cooling_until": if st_eff == PoolStatus::Cooling { st.cooling_until } else { 0 },
                    "risk_strikes": st.risk_strikes,
                    "remaining": st.remaining(),
                    "quota_at": st.quota_at,
                    "use_count": st.use_count,
                    "fail_count": st.fail_count,
                    "last_error": st.last_error,
                    "inflight": self.inflight_of(id),
                }))
            })
            .collect()
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pool_with(ids: &[(&str, f64, usize)], dir: &std::path::Path) -> Pool {
        let p = Pool::new(dir.join("gw-state.json"));
        for (id, rem, inf) in ids {
            p.with_state(id, |st| {
                st.quota = Some(json!({ "remaining": rem }));
            });
            for _ in 0..*inf {
                assert!(p.acquire_slot(id));
            }
        }
        p
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn select_skips_saturated_and_cooling() {
        let dir = std::env::temp_dir().join(format!("gwtest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = pool_with(&[("a", 100.0, ACCOUNT_CONCURRENCY), ("b", 10.0, 0)], &dir);
        let got = p.select(&ids(&["a", "b"]), &std::collections::HashSet::new(), None);
        assert_eq!(got, Some("b".into()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn select_prefers_richer_within_tolerance_band() {
        let dir = std::env::temp_dir().join(format!("gwtest2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 100 vs 103 在 5% 容差带内 → 同档，轮询均摊；101 vs 200 → 明显更富者胜
        let p = pool_with(&[("a", 100.0, 0), ("b", 103.0, 0)], &dir);
        let first = p.select(&ids(&["a", "b"]), &std::collections::HashSet::new(), None);
        assert!(first.is_some());
        let p2 = pool_with(&[("a", 101.0, 1), ("b", 200.0, 1)], &dir);
        let got = p2.select(&ids(&["a", "b"]), &std::collections::HashSet::new(), None);
        assert_eq!(got, Some("b".into()), "同负载下显著更富者优先");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn exhausted_never_selectable_but_cooling_recovers() {
        let dir = std::env::temp_dir().join(format!("gwtest3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = Pool::new(dir.join("gw-state.json"));
        p.with_state("x", |st| st.status = Some(PoolStatus::Exhausted));
        assert!(!p.snapshot_of("x").unwrap().is_selectable(now_ms()));
        p.with_state("y", |st| {
            st.status = Some(PoolStatus::Cooling);
            st.cooling_until = now_ms() - 1;
        });
        assert!(p.snapshot_of("y").unwrap().is_selectable(now_ms()), "冷却到期惰性恢复");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn state_persists_across_instances() {
        let dir = std::env::temp_dir().join(format!("gwtest4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        {
            let p = Pool::new(dir.join("gw-state.json"));
            p.with_state("k", |st| {
                st.status = Some(PoolStatus::Invalid);
                st.risk_strikes = 3;
            });
            p.persist();
        }
        let p2 = Pool::new(dir.join("gw-state.json"));
        let st = p2.snapshot_of("k").unwrap();
        assert_eq!(st.status_or_active(), PoolStatus::Invalid);
        assert_eq!(st.risk_strikes, 3);
        std::fs::remove_dir_all(&dir).ok();
    }
}
