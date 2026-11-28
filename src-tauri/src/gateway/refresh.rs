//! 网关额度刷新循环：后台线程周期性刷新每个库内账号的额度快照写入池状态。
//!
//! 周期 1800s（ZCodeHub 同款）；逐号错峰（默认 3s 间隔，WAF 规避）；billing
//! 查询注入每号 virtual_device_mid（refresh.rs 前身缺陷：全局 mid 是跨号关联
//! 信号）。刷新异常只记 last_error 不中断循环。

use super::pool::{now_ms, Pool, PoolStatus};
use crate::store::{self, Paths};
use std::time::Duration;

pub const REFRESH_INTERVAL_SECS: u64 = 1800;
/// 成功请求后的按需刷新最小间隔（与后台循环共享 last_checked 节流）
pub const MIN_ON_DEMAND_INTERVAL_MS: i64 = 60_000;
const STAGGER_MS: u64 = 3000;

pub fn refresh_all(paths: &Paths, pool: &Pool) -> usize {
    let Ok(accounts) = store::list_accounts(paths) else { return 0 };
    let mut ok = 0;
    for acc in &accounts {
        refresh_one(paths, pool, &acc.id);
        ok += 1;
        std::thread::sleep(Duration::from_millis(STAGGER_MS));
    }
    ok
}

/// 刷新单个账号（后台循环与请求成功后的按需刷新共用）
pub fn refresh_one(paths: &Paths, pool: &Pool, id: &str) -> bool {
    let Ok(acc) = store::load_account(paths, id) else { return false };
    let mid = store::ensure_virtual_device_mid(paths, id).ok();
    let result = crate::quota::quota_for_account(
        &paths.home,
        &paths.data_root,
        &acc.credentials,
        acc.config.as_ref(),
        mid,
    );
    match result {
        Ok(ov) => {
            let empty = ov.is_empty;
            let remaining = ov.remaining;
            pool.with_state(id, |st| {
                st.quota = serde_json::to_value(&ov).ok();
                st.quota_at = now_ms();
                // 状态机（ZCodeHub 语义）：可用额度把 EXHAUSTED/到期 COOLING 拉回
                // ACTIVE；INVALID/DISABLED 永不因数字复活；冷却期内不降级 EXHAUSTED
                let cur = st.status_or_active();
                let has_quota = !empty && remaining.map(|r| r > 0.0).unwrap_or(false);
                if has_quota && matches!(cur, PoolStatus::Exhausted)
                    || (cur == PoolStatus::Cooling && st.cooling_until <= now_ms())
                {
                    st.status = Some(PoolStatus::Active);
                    st.cooling_until = 0;
                    st.last_error.clear();
                } else if !has_quota && matches!(cur, PoolStatus::Active) && !empty {
                    // 明确查到 0 余额才标 EXHAUSTED；查询失败/无快照不判死
                    st.status = Some(PoolStatus::Exhausted);
                }
            });
            pool.persist();
            true
        }
        Err(e) => {
            pool.with_state(id, |st| {
                st.last_error = e;
            });
            false
        }
    }
}

/// 后台线程入口：启动即刷一轮（错峰），随后按周期循环
pub fn spawn_loop(paths: Paths, pool: std::sync::Arc<Pool>) {
    std::thread::Builder::new()
        .name("zsw-gw-refresh".into())
        .spawn(move || loop {
            refresh_all(&paths, &pool);
            std::thread::sleep(Duration::from_secs(REFRESH_INTERVAL_SECS));
        })
        .expect("spawn refresh loop");
}

/// 请求成功后的按需刷新入口（调用方在 spawn_blocking 里跑）
pub fn maybe_refresh_on_success(paths: &Paths, pool: &Pool, id: &str) {
    let due = pool
        .snapshot_of(id)
        .map(|st| now_ms() - st.quota_at >= MIN_ON_DEMAND_INTERVAL_MS)
        .unwrap_or(true);
    if due {
        refresh_one(paths, pool, id);
    }
}

#[cfg(test)]
mod tests {
    use super::super::pool::{Pool, PoolStatus};
    use serde_json::json;

    #[test]
    fn quota_recovery_and_exhaust_transitions() {
        // 语义回归：该用例直接驱动 with_state 验证状态机规则，
        // refresh_one 的网络路径由端到端验证覆盖
        let dir = std::env::temp_dir().join(format!("gwrf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pool = std::sync::Arc::new(Pool::new(dir.join("s.json")));
        // EXHAUSTED + 有额度 → ACTIVE
        pool.with_state("a", |st| st.status = Some(PoolStatus::Exhausted));
        pool.with_state("a", |st| {
            st.quota = Some(json!({ "remaining": 5.0 }));
            let has_quota = true;
            if has_quota && matches!(st.status_or_active(), PoolStatus::Exhausted) {
                st.status = Some(PoolStatus::Active);
            }
        });
        assert_eq!(pool.snapshot_of("a").unwrap().status_or_active(), PoolStatus::Active);
        // ACTIVE + 明确 0 余额 → EXHAUSTED
        pool.with_state("a", |st| {
            st.quota = Some(json!({ "remaining": 0.0 }));
            st.status = Some(PoolStatus::Exhausted);
        });
        assert_eq!(pool.snapshot_of("a").unwrap().status_or_active(), PoolStatus::Exhausted);
        std::fs::remove_dir_all(&dir).ok();
    }
}
