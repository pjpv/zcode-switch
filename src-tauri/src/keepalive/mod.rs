//! Opt-in recovery of newly observed local CAPTCHA interruptions.
mod log_reader;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
mod worker;

use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex, OnceLock,
};

static ENABLED: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static FINISH_ON_CANCEL: AtomicBool = AtomicBool::new(true);
static LAUNCH_COMMITTED: AtomicBool = AtomicBool::new(false);
static STATUS: OnceLock<Mutex<Status>> = OnceLock::new();

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub supported: bool,
    pub enabled: bool,
    pub phase: String,
    pub pending: usize,
    pub recovered: usize,
    pub attempt: u32,
    pub next_retry_at: Option<i64>,
    pub reason: Option<String>,
}
impl Default for Status {
    fn default() -> Self {
        Self {
            supported: cfg!(windows),
            enabled: false,
            phase: "off".into(),
            pending: 0,
            recovered: 0,
            attempt: 0,
            next_retry_at: None,
            reason: None,
        }
    }
}
pub fn status() -> Status {
    let mut s = STATUS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    s.enabled = ENABLED.load(Ordering::SeqCst);
    s
}
fn publish(mut s: Status) {
    s.enabled = ENABLED.load(Ordering::SeqCst);
    *STATUS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = s;
}
/// Manual account/lifecycle actions cancel recovery before waiting for STORE_LOCK.
pub fn cancel() {
    ENABLED.store(false, Ordering::SeqCst);
    GENERATION.fetch_add(1, Ordering::SeqCst);
    publish(Status::default());
}
pub fn configure(enabled: bool) -> Result<Status, String> {
    FINISH_ON_CANCEL.store(true, Ordering::SeqCst);
    configure_inner(enabled)
}
pub fn manual_action() -> Result<Status, String> {
    cancel_manual();
    configure_inner(false)
}
pub fn cancel_manual() {
    FINISH_ON_CANCEL.store(false, Ordering::SeqCst);
    LAUNCH_COMMITTED.store(false, Ordering::SeqCst);
    cancel();
}
fn configure_inner(enabled: bool) -> Result<Status, String> {
    if !enabled {
        cancel();
    }
    #[cfg(not(windows))]
    if enabled {
        return Err("Keepalive is available on Windows only".into());
    }
    #[cfg(windows)]
    if enabled {
        worker::enable()?;
    }
    let _guard = crate::store_guard();
    let paths = crate::store::Paths::detect();
    let mut settings = crate::store::load_settings(&paths);
    settings.keepalive_enabled = Some(enabled);
    if let Err(e) = crate::store::save_settings(&paths, &settings) {
        cancel();
        return Err(e);
    }
    Ok(status())
}
pub fn start() {
    #[cfg(windows)]
    {
        worker::start();
        if crate::store::load_settings(&crate::store::Paths::detect())
            .keepalive_enabled
            .unwrap_or(false)
        {
            if let Err(_e) = worker::enable() {
                let mut s = Status::default();
                s.phase = "error".into();
                s.reason = Some("setup".into());
                publish(s);
            }
        }
    }
}

fn retry_delay_ms(attempt: u32) -> i64 {
    // Continuous recovery, with a finite frequency cap rather than an attempt cap.
    (15_000_i64.saturating_mul(1_i64 << attempt.saturating_sub(1).min(5))).min(300_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retry_frequency_is_bounded_without_an_attempt_limit() {
        assert_eq!(retry_delay_ms(1), 15_000);
        assert_eq!(retry_delay_ms(2), 30_000);
        assert_eq!(retry_delay_ms(100_000), 300_000);
    }
}
