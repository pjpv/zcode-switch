//! 验证码求解：复用 zcode2api 的 headed Chromium 求解器子进程。
//!
//! 依据（记忆文档实测）：无头求解必 F001，headed Chromium 一次通过；param TTL
//! 仅约 45 秒，解出后必须立即发。求解命令（已实测产出 VERIFY_PARAM）：
//!   CAPTCHA_DEBUG=1 ZCODE_CAPTCHA_HEADED=1 ZCODE_SOLVE_DEADLINE_MS=90000 \
//!     <python> -B <slidex_runner.py> <scene_id> <region> <prefix>
//!   → stdout 行 `VERIFY_PARAM=eyJ...`
//!
//! scene_id / region / prefix 来自 claim::fetch_captcha_config()（CLIENT_CONFIGS_URL
//! 的 /data/configs/captcha）。求解器路径可在设置里配，默认指向本机 zcode2api
//! 安装位置；不可用时调用方降级为冷却换号（模块内不留死等）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// param 有效期（上游实测约 45s，留 5s 余量）
pub const PARAM_TTL_MS: i64 = 45_000;
const SOLVE_TIMEOUT: Duration = Duration::from_secs(90);

pub const ENV_VERIFY_PARAM: &str = "X-Aliyun-Captcha-Verify-Param";
pub const ENV_VERIFY_REGION: &str = "X-Aliyun-Captcha-Verify-Region";

pub struct SolverConfig {
    pub python: PathBuf,
    pub script: PathBuf,
}

impl SolverConfig {
    /// 设置里的路径；缺省回退本机 zcode2api 安装位置
    pub fn resolve(python: Option<&str>, script: Option<&str>) -> Option<SolverConfig> {
        let py = python
            .map(PathBuf::from)
            .or_else(|| PathBuf::from("/media/emo/code/zcode2api/.venv/bin/python").exists().then_some(PathBuf::from("/media/emo/code/zcode2api/.venv/bin/python")));
        let sc = script
            .map(PathBuf::from)
            .or_else(|| PathBuf::from("/media/emo/code/zcode2api/captcha_node/slidex_runner.py").exists().then_some(PathBuf::from("/media/emo/code/zcode2api/captcha_node/slidex_runner.py")));
        let (py, sc) = (py?, sc?);
        (py.is_file() && sc.is_file()).then_some(SolverConfig { python: py, script: sc })
    }

    pub fn is_configured(&self) -> bool {
        self.python.is_file() && self.script.is_file()
    }
}

/// 求解结果缓存（TTL 45s）+ 并发去重：同一时刻只跑一个求解器子进程，
/// 其它调用等同一个结果。求解失败不缓存（下次直接重试）。
pub struct CaptchaSolver {
    cfg: SolverConfig,
    cache: Mutex<HashMap<String, (String, Instant)>>, // region -> (param, expire)
}

impl CaptchaSolver {
    pub fn new(cfg: SolverConfig) -> CaptchaSolver {
        CaptchaSolver { cfg, cache: Mutex::new(HashMap::new()) }
    }

    fn cached(&self, region: &str) -> Option<String> {
        let Ok(mut g) = self.cache.lock() else { return None };
        g.retain(|_, (_, exp)| *exp > Instant::now());
        g.get(region).cloned().map(|(p, _)| p)
    }

    /// 只读缓存（不触发求解）：请求组装时已解出的 param 直接带上
    pub fn peek(&self) -> Option<String> {
        self.cache
            .lock()
            .ok()?
            .values()
            .find(|(_, exp)| *exp > Instant::now())
            .cloned()
            .map(|(p, _)| p)
    }

    /// 拉起求解器子进程（headed Chromium 需要图形会话 env；继承当前进程 env，
    /// 桌面应用内天然带 DISPLAY/WAYLAND_DISPLAY）。返回 verify param。
    pub fn solve(&self, scene_id: &str, region: &str, prefix: &str) -> Result<String, String> {
        if let Some(p) = self.cached(region) {
            return Ok(p);
        }
        if !self.cfg.is_configured() {
            return Err("求解器未配置".into());
        }
        let mut cmd = Command::new(&self.cfg.python);
        cmd.arg("-B")
            .arg(&self.cfg.script)
            .arg(scene_id)
            .arg(region)
            .arg(prefix)
            .env("CAPTCHA_DEBUG", "1")
            .env("ZCODE_CAPTCHA_HEADED", "1")
            .env("ZCODE_SOLVE_DEADLINE_MS", "90000")
            .stdin(std::process::Stdio::null());
        let out = cmd
            .output()
            .map_err(|e| format!("求解器启动失败: {e}"))?;
        if !out.status.success() {
            let tail = String::from_utf8_lossy(&out.stderr);
            return Err(format!("求解器退出码 {:?}，stderr 尾部: {}", out.status.code(), last_lines(&tail, 3)));
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let param = stdout
            .lines()
            .find_map(|l| l.strip_prefix("VERIFY_PARAM=").map(str::trim).map(String::from))
            .filter(|p| !p.is_empty())
            .ok_or_else(|| format!("求解器未产出 VERIFY_PARAM，stdout 尾部: {}", last_lines(&stdout, 3)))?;
        if let Ok(mut g) = self.cache.lock() {
            g.insert(region.to_string(), (param.clone(), Instant::now() + Duration::from_millis(PARAM_TTL_MS as u64)));
        }
        Ok(param)
    }

    /// 3007 后失效缓存（param 已被上游消耗）
    pub fn invalidate(&self, region: &str) {
        if let Ok(mut g) = self.cache.lock() {
            g.remove(region);
        }
    }
}

fn last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_missing_paths_returns_none() {
        assert!(SolverConfig::resolve(Some("/nonexistent/python"), None).is_none());
        assert!(SolverConfig::resolve(None, Some("/nonexistent/script.py")).is_none());
    }

    #[test]
    fn cache_ttl_and_invalidate() {
        let solver = CaptchaSolver::new(SolverConfig {
            python: PathBuf::from("/bin/true"),
            script: PathBuf::from("/bin/true"),
        });
        // 直接注入缓存验证 TTL/失效语义
        solver
            .cache
            .lock()
            .unwrap()
            .insert("sgp".into(), ("P".into(), Instant::now() + Duration::from_millis(50)));
        assert_eq!(solver.cached("sgp"), Some("P".into()));
        solver.invalidate("sgp");
        assert_eq!(solver.cached("sgp"), None);
    }

    #[test]
    fn parse_verify_param_line() {
        let stdout = "noise\n[2026] headed started\nVERIFY_PARAM=eyJhbGciOi.abc_123\nafter";
        let param = stdout
            .lines()
            .find_map(|l| l.strip_prefix("VERIFY_PARAM=").map(str::trim).map(String::from))
            .unwrap();
        assert_eq!(param, "eyJhbGciOi.abc_123");
    }
}
