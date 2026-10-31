use super::{
    log_reader::{EventKind, LogReader, ObservedEvent},
    windows::{self, Instance, Renderer},
    *,
};
use crate::store::{self, Paths};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::mpsc::{self, Sender},
    time::Duration,
};

static COMMANDS: OnceLock<Sender<Context>> = OnceLock::new();
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Target {
    session_id: String,
    workspace_path: String,
}
struct Pending {
    target: Target,
    baseline_turn: Option<String>,
    detected_at: i64,
    command_id: String,
    submitted_at: Option<i64>,
}
struct Context {
    generation: u64,
    executable: String,
    data_root: PathBuf,
    account: String,
    reader: LogReader,
    pending: BTreeMap<String, Pending>,
    active: HashMap<String, (String, i64)>,
    instance: Option<Instance>,
    restarted_at: Option<i64>,
    next_retry: i64,
    needs_restart: bool,
    launch_pending: bool,
    control_ready: bool,
    state: Status,
}

fn account(paths: &Paths) -> Result<String, String> {
    let credentials =
        store::read_live(paths)?.ok_or("Sign in to ZCode before enabling keepalive")?;
    let identity = crate::zcrypto::account_identity(&credentials, &paths.home);
    let user = identity
        .user_id
        .ok_or("ZCode account identity could not be verified")?;
    Ok(format!(
        "{:x}",
        Sha256::digest(format!("{}\0{}", identity.provider, user))
    ))
}

pub fn enable() -> Result<(), String> {
    if LAUNCH_COMMITTED.load(Ordering::SeqCst) {
        return Err("ZCode is finishing a committed restart; wait or start it manually".into());
    }
    let paths = Paths::detect();
    let (executable, ok) = store::effective_zcode_path(&paths);
    if !ok {
        return Err("Select a valid ZCode installation before enabling keepalive".into());
    }
    let account = account(&paths)?;
    let reader = LogReader::new(
        paths.zcode_dir().join("cli/log"),
        chrono::Utc::now().timestamp_millis(),
    )?;
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let state = Status {
        enabled: true,
        phase: "watching".into(),
        ..Status::default()
    };
    let context = Context {
        generation,
        executable,
        data_root: paths.data_root,
        account,
        reader,
        pending: BTreeMap::new(),
        active: HashMap::new(),
        instance: None,
        restarted_at: None,
        next_retry: 0,
        needs_restart: false,
        launch_pending: false,
        control_ready: false,
        state: state.clone(),
    };
    ENABLED.store(true, Ordering::SeqCst);
    COMMANDS
        .get()
        .ok_or("Keepalive worker is not ready")?
        .send(context)
        .map_err(|_| "Keepalive worker is unavailable")?;
    publish(state);
    Ok(())
}

pub fn start() {
    let (tx, rx) = mpsc::channel::<Context>();
    if COMMANDS.set(tx).is_err() {
        return;
    }
    std::thread::spawn(move || {
        let mut current: Option<Context> = None;
        loop {
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(next) => current = Some(next),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            let Some(c) = current.as_mut() else { continue };
            if !c.enabled() {
                if c.launch_pending && FINISH_ON_CANCEL.load(Ordering::SeqCst) {
                    if chrono::Utc::now().timestamp_millis() < c.next_retry {
                        continue;
                    }
                    let _guard = crate::store_guard();
                    if !FINISH_ON_CANCEL.load(Ordering::SeqCst) || ENABLED.load(Ordering::SeqCst) {
                        current = None;
                        continue;
                    }
                    // Cancellation stops continuation, but still completes the
                    // already committed shutdown when launching is possible.
                    let executable = c.executable.clone();
                    let completed = match windows::inspect(&executable, false) {
                        Ok(Some(_)) => Ok(()),
                        Ok(None) => c.finish_launch(|| windows::start(&executable)),
                        Err(e) => Err(e),
                    };
                    if !c.finish_cancelled_attempt(completed) {
                        continue;
                    }
                }
                current = None;
                continue;
            }
            if let Err(_e) = c.tick() {
                c.state.phase = "error".into();
                c.state.reason = Some("unavailable".into());
                c.next_retry = chrono::Utc::now().timestamp_millis() + 15_000;
            }
            if c.enabled() {
                c.state.pending = c.pending.len();
                c.state.next_retry_at =
                    (c.next_retry > chrono::Utc::now().timestamp_millis()).then_some(c.next_retry);
                publish(c.state.clone());
            }
        }
    });
}

fn target(paths: &Paths, id: &str, collateral: bool) -> Result<Option<Target>, String> {
    if id.is_empty() || id.len() > 200 {
        return Ok(None);
    }
    let db = Connection::open_with_flags(
        paths.zcode_dir().join("v2/tasks-index.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| "Cannot read ZCode task index")?;
    db.busy_timeout(Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    let rows = db.prepare("SELECT workspace_path FROM tasks WHERE task_id=?1 AND deleted=0 AND archived=0 AND (workspace_identity IS NULL OR workspace_identity='') AND (task_status='running' OR (?2=0 AND task_status='error'))").map_err(|_| "Unsupported ZCode task index")?
        .query_map(rusqlite::params![id, collateral], |row| row.get::<_, String>(0)).map_err(|_| "Cannot read ZCode task index")?
        .collect::<Result<Vec<_>, _>>().map_err(|_| "Cannot read ZCode task index")?;
    if rows.len() != 1 || !PathBuf::from(&rows[0]).is_absolute() {
        return Ok(None);
    }
    Ok(Some(Target {
        session_id: id.into(),
        workspace_path: rows[0].clone(),
    }))
}

impl Context {
    fn finish_cancelled_attempt(&mut self, result: Result<(), String>) -> bool {
        if result.is_err() {
            self.state.attempt = self.state.attempt.saturating_add(1);
            self.next_retry =
                chrono::Utc::now().timestamp_millis() + retry_delay_ms(self.state.attempt);
            let mut state = Status::default();
            state.reason = Some("restart_incomplete".into());
            state.next_retry_at = Some(self.next_retry);
            publish(state);
            return false;
        }
        self.launch_pending = false;
        LAUNCH_COMMITTED.store(false, Ordering::SeqCst);
        publish(Status::default());
        true
    }
    fn finish_launch(
        &mut self,
        start: impl FnOnce() -> Result<Instance, String>,
    ) -> Result<(), String> {
        let instance = start()?;
        self.instance = Some(instance);
        self.launch_pending = false;
        LAUNCH_COMMITTED.store(false, Ordering::SeqCst);
        self.restarted_at = Some(chrono::Utc::now().timestamp_millis());
        Ok(())
    }
    fn enabled(&self) -> bool {
        ENABLED.load(Ordering::SeqCst) && GENERATION.load(Ordering::SeqCst) == self.generation
    }
    fn observe_snapshot(&mut self, state: &serde_json::Value, now: i64) {
        let Some(id) = state["sessionId"].as_str() else {
            return;
        };
        let Some(p) = self.pending.get(id) else {
            return;
        };
        let status = state["status"].as_str().unwrap_or("unavailable");
        let changed = state["turnId"]
            .as_str()
            .is_some_and(|turn| p.baseline_turn.as_deref() != Some(turn));
        if matches!(status, "paused" | "waiting") {
            self.pending.remove(id);
        } else if status == "completed" {
            if changed {
                self.state.recovered += 1;
            }
            self.pending.remove(id);
        } else if status == "error" && changed {
            self.pending.remove(id);
        } else if matches!(status, "error" | "idle")
            && p.submitted_at.is_some_and(|sent| now - sent > 60_000)
        {
            self.needs_restart = true;
            self.next_retry = now + retry_delay_ms(self.state.attempt);
        }
    }
    fn observe(&mut self, paths: &Paths, event: ObservedEvent) -> Result<(), String> {
        let id = event.session_id.clone();
        match event.kind {
            EventKind::CaptchaFailure => {
                self.active.remove(&id);
                if let Some(t) = target(paths, &id, false)? {
                    let p = Pending {
                        target: t,
                        baseline_turn: Some(event.turn_id),
                        detected_at: event.at_ms,
                        command_id: uuid::Uuid::new_v4().to_string(),
                        submitted_at: None,
                    };
                    self.pending.insert(id, p);
                    self.needs_restart = true;
                    // A fresh failure after real progress starts a new recovery cycle.
                    self.next_retry = if self.state.attempt == 0 {
                        0
                    } else {
                        chrono::Utc::now().timestamp_millis() + retry_delay_ms(self.state.attempt)
                    };
                }
            }
            EventKind::TurnStarted => {
                if self.pending.get(&id).is_some_and(|p| {
                    p.submitted_at.is_none()
                        && event.at_ms > p.detected_at
                        && p.baseline_turn.as_deref() != Some(&event.turn_id)
                }) {
                    self.pending.remove(&id);
                }
                self.active.insert(id, (event.turn_id, event.at_ms));
            }
            EventKind::Progress | EventKind::TurnCompleted => {
                if self.pending.get(&id).is_some_and(|p| {
                    p.submitted_at.is_some_and(|sent| event.at_ms >= sent)
                        && p.baseline_turn.as_deref() != Some(&event.turn_id)
                }) {
                    self.pending.remove(&id);
                    self.state.recovered += 1;
                }
                if event.kind == EventKind::TurnCompleted {
                    self.active.remove(&id);
                } else {
                    self.active.insert(id, (event.turn_id, event.at_ms));
                }
            }
            EventKind::OtherFailure => {
                self.active.remove(&id);
                self.pending.remove(&id);
            }
        }
        // Bound in-memory observation to recent local activity.
        let floor = chrono::Utc::now().timestamp_millis() - 24 * 60 * 60 * 1000;
        self.active.retain(|_, (_, at)| *at >= floor);
        if self.active.len() > 512 {
            self.active.clear();
        }
        Ok(())
    }
    fn tick(&mut self) -> Result<(), String> {
        let paths = Paths::detect();
        if paths.data_root != self.data_root
            || store::effective_zcode_path(&paths).0 != self.executable
            || account(&paths)? != self.account
        {
            manual_action()?;
            let mut s = Status::default();
            s.reason = Some("identity_changed".into());
            publish(s);
            return Ok(());
        }
        for event in self.reader.poll()? {
            self.observe(&paths, event)?;
        }
        if self.pending.is_empty() && !self.launch_pending {
            self.needs_restart = false;
            self.state.phase = "watching".into();
            self.state.reason = None;
            self.state.attempt = 0;
            self.next_retry = 0;
            self.restarted_at = None;
            return Ok(());
        }
        let now = chrono::Utc::now().timestamp_millis();
        if now < self.next_retry {
            self.state.phase = "retrying".into();
            return Ok(());
        }
        let Ok(_guard) = crate::STORE_LOCK.try_lock() else {
            return Ok(());
        };
        if !self.enabled() {
            return Ok(());
        }
        if self.launch_pending {
            if let Some(existing) = windows::inspect(&self.executable, false)? {
                if self
                    .instance
                    .as_ref()
                    .is_some_and(|old| windows::same_instance(old, &existing))
                {
                    self.launch_pending = false;
                    LAUNCH_COMMITTED.store(false, Ordering::SeqCst);
                    self.needs_restart = true;
                    self.next_retry = now + retry_delay_ms(self.state.attempt);
                } else {
                    // A replacement outside our launch receipt must not receive
                    // recovery commands merely because it uses the same path.
                    self.pending.clear();
                    self.active.clear();
                    self.launch_pending = false;
                    LAUNCH_COMMITTED.store(false, Ordering::SeqCst);
                    self.instance = None;
                }
                return Ok(());
            }
            self.state.phase = "connecting".into();
            let executable = self.executable.clone();
            self.finish_launch(|| windows::start(&executable))?;
            return Ok(());
        }
        if self.needs_restart {
            let Some(instance) = windows::inspect(&self.executable, true)? else {
                // A manual close is not permission to resurrect ZCode.
                self.state.phase = "waiting_app".into();
                return Ok(());
            };
            if let Some(old) = &self.instance {
                if !windows::same_instance(old, &instance) {
                    self.pending.clear();
                    self.active.clear();
                    self.instance = None;
                    return Ok(());
                }
            }
            // Include only activity observed during this opt-in period. Inspect
            // after restart still excludes permissions, questions and manual stop.
            for (id, (turn, at)) in &self.active {
                if !self.pending.contains_key(id) {
                    if let Some(t) = target(&paths, id, true)? {
                        self.pending.insert(
                            id.clone(),
                            Pending {
                                target: t,
                                baseline_turn: Some(turn.clone()),
                                detected_at: *at,
                                command_id: uuid::Uuid::new_v4().to_string(),
                                submitted_at: None,
                            },
                        );
                    }
                }
            }
            if self.pending.len() > 64 {
                self.state.phase = "error".into();
                self.state.reason = Some("too_many".into());
                return Ok(());
            }
            self.state.phase = "restarting".into();
            publish(self.state.clone());
            if !self.enabled() {
                return Ok(());
            }
            // Persist the in-memory shutdown intent before the helper: its
            // response can be lost after it has already stopped the process.
            self.launch_pending = true;
            LAUNCH_COMMITTED.store(true, Ordering::SeqCst);
            self.instance = Some(instance.clone());
            windows::stop(&instance)?;
            self.instance = None;
            self.needs_restart = false;
            self.control_ready = false;
            self.state.attempt = self.state.attempt.saturating_add(1);
            for p in self.pending.values_mut() {
                p.submitted_at = None;
                p.command_id = uuid::Uuid::new_v4().to_string();
            }
            // Complete a committed restart even if switched off during shutdown;
            // cancellation prevents all subsequent continuation messages.
            let executable = self.executable.clone();
            self.finish_launch(|| windows::start(&executable))?;
            self.state.phase = "connecting".into();
            return Ok(());
        }
        let Some(current) = windows::inspect(&self.executable, true)? else {
            if !self.control_ready && self.restarted_at.is_some() {
                self.launch_pending = true;
                LAUNCH_COMMITTED.store(true, Ordering::SeqCst);
                self.instance = None;
                self.next_retry = now + retry_delay_ms(self.state.attempt);
            }
            self.state.phase = "waiting_app".into();
            return Ok(());
        };
        if self
            .instance
            .as_ref()
            .is_none_or(|i| !windows::same_instance(i, &current))
        {
            self.pending.clear();
            self.active.clear();
            self.instance = None;
            return Ok(());
        }
        let renderer = match Renderer::discover(&current) {
            Ok(r) => r,
            Err(_) => {
                self.state.phase = "connecting".into();
                if now - self.restarted_at.unwrap_or(now) > 90_000 {
                    self.state.phase = "error".into();
                    self.state.reason = Some("control_unavailable".into());
                    if self.pending.values().all(|p| p.submitted_at.is_none()) {
                        self.needs_restart = true;
                        self.next_retry = now + retry_delay_ms(self.state.attempt);
                    }
                }
                return Ok(());
            }
        };
        renderer.evaluate(include_str!("renderer.js"))?;
        let probe = renderer.evaluate("globalThis.__zswitchKeepalive.probe()")?;
        if probe["supported"] != true {
            self.state.phase = "error".into();
            self.state.reason = Some("unsupported_version".into());
            self.next_retry = now + 30_000;
            return Ok(());
        }
        self.control_ready = true;
        self.state.phase = "resuming".into();
        self.state.reason = None;
        let observing: Vec<_> = self
            .pending
            .values()
            .filter(|p| p.submitted_at.is_some())
            .map(|p| p.target.clone())
            .collect();
        if !observing.is_empty() {
            let states = renderer.evaluate(&format!(
                "globalThis.__zswitchKeepalive.inspect({})",
                serde_json::to_string(&observing).unwrap()
            ))?;
            if let Some(states) = states.as_array() {
                for state in states {
                    self.observe_snapshot(state, now);
                }
            }
        }
        let ids: Vec<_> = self.pending.keys().cloned().collect();
        for id in ids {
            if !self.enabled() {
                break;
            }
            let p = self.pending.get(&id).unwrap();
            if p.submitted_at.is_some() {
                continue;
            }
            let request = json!({"sessionId":p.target.session_id,"workspacePath":p.target.workspace_path,"commandId":p.command_id,"issuedAt":now,"recoveryStartedAt":p.detected_at,"baselineTurnId":p.baseline_turn});
            let result =
                renderer.evaluate(&format!("globalThis.__zswitchKeepalive.resume({request})"));
            match result.as_ref().ok().and_then(|v| v["status"].as_str()) {
                Some(
                    "accepted" | "duplicate" | "uncertain" | "unknown_ack" | "already_running"
                    | "rejected" | "stale" | "noop" | "failed",
                ) => {
                    self.pending.get_mut(&id).unwrap().submitted_at = Some(now);
                }
                Some("completed" | "blocked" | "changed" | "identity_unavailable") => {
                    self.pending.remove(&id);
                }
                _ => {
                    self.state.reason = Some("resume_unavailable".into());
                    self.next_retry = now + 15_000;
                }
            }
        }
        self.state.phase = "observing".into();
        // Never restart merely for slow progress. A fresh CAPTCHA failure or
        // a verified idle/error snapshot is required; unknown ACKs are observed.
        if self.needs_restart {
            self.next_retry = now + retry_delay_ms(self.state.attempt);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        paths: Paths,
        context: Context,
    }
    impl Fixture {
        fn new() -> Self {
            let home =
                std::env::temp_dir().join(format!("zswitch-worker-{}", uuid::Uuid::new_v4()));
            let paths = Paths {
                home: home.clone(),
                data_root: home.clone(),
            };
            std::fs::create_dir_all(paths.zcode_dir().join("v2")).unwrap();
            let db = Connection::open(paths.zcode_dir().join("v2/tasks-index.sqlite")).unwrap();
            db.execute_batch("CREATE TABLE tasks(task_id TEXT, workspace_path TEXT, workspace_identity TEXT, task_status TEXT,deleted INTEGER DEFAULT 0,archived INTEGER DEFAULT 0); INSERT INTO tasks VALUES ('sess-a','C:\\work',NULL,'error',0,0),('sess-b','C:\\work',NULL,'running',0,0),('sess-c','C:\\work','ssh:test','running',0,0),('sess-d','C:\\work',NULL,'completed',0,0);").unwrap();
            let context = Context {
                generation: u64::MAX,
                executable: "fixture.exe".into(),
                data_root: home,
                account: "fixture".into(),
                reader: LogReader::new(paths.zcode_dir().join("cli/log"), 0).unwrap(),
                pending: BTreeMap::new(),
                active: HashMap::new(),
                instance: None,
                restarted_at: None,
                next_retry: 0,
                needs_restart: false,
                launch_pending: false,
                control_ready: false,
                state: Status::default(),
            };
            Self { paths, context }
        }
        fn failure(&mut self) {
            self.context
                .observe(
                    &self.paths,
                    ObservedEvent {
                        session_id: "sess-a".into(),
                        turn_id: "turn-original".into(),
                        at_ms: chrono::Utc::now().timestamp_millis(),
                        kind: EventKind::CaptchaFailure,
                    },
                )
                .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.paths.home);
        }
    }
    #[test]
    fn collateral_requires_running_and_exact_local_index_membership() {
        let f = Fixture::new();
        assert!(target(&f.paths, "sess-a", false).unwrap().is_some());
        assert!(target(&f.paths, "sess-a", true).unwrap().is_none());
        assert!(target(&f.paths, "sess-b", true).unwrap().is_some());
        for id in ["sess-c", "sess-d", "missing"] {
            assert!(target(&f.paths, id, false).unwrap().is_none());
        }
    }
    #[test]
    fn user_started_new_turn_cancels_captured_recovery() {
        let mut f = Fixture::new();
        f.failure();
        let at = f.context.pending["sess-a"].detected_at + 1;
        f.context
            .observe(
                &f.paths,
                ObservedEvent {
                    session_id: "sess-a".into(),
                    turn_id: "turn-user".into(),
                    at_ms: at,
                    kind: EventKind::TurnStarted,
                },
            )
            .unwrap();
        assert!(f.context.pending.is_empty());
    }
    #[test]
    fn acknowledged_send_requires_progress_from_a_new_turn() {
        let mut f = Fixture::new();
        f.failure();
        let at = f.context.pending["sess-a"].detected_at + 1;
        f.context.pending.get_mut("sess-a").unwrap().submitted_at = Some(at);
        for (turn, kind) in [
            ("turn-original", EventKind::Progress),
            ("turn-new", EventKind::TurnStarted),
        ] {
            f.context
                .observe(
                    &f.paths,
                    ObservedEvent {
                        session_id: "sess-a".into(),
                        turn_id: turn.into(),
                        at_ms: at,
                        kind,
                    },
                )
                .unwrap();
            assert_eq!(f.context.state.recovered, 0);
            assert_eq!(f.context.pending.len(), 1);
        }
        f.context
            .observe(
                &f.paths,
                ObservedEvent {
                    session_id: "sess-a".into(),
                    turn_id: "turn-new".into(),
                    at_ms: at + 1,
                    kind: EventKind::Progress,
                },
            )
            .unwrap();
        assert_eq!(f.context.state.recovered, 1);
        assert!(f.context.pending.is_empty());
    }
    #[test]
    fn ambiguous_ack_retries_only_after_runtime_proves_idle() {
        let mut f = Fixture::new();
        f.failure();
        f.context.needs_restart = false;
        f.context.state.attempt = 3;
        f.context.pending.get_mut("sess-a").unwrap().submitted_at = Some(1);
        f.context.observe_snapshot(
            &json!({"sessionId":"sess-a","status":"running","turnId":"turn-new"}),
            1_000_000,
        );
        assert!(!f.context.needs_restart);
        f.context.observe_snapshot(
            &json!({"sessionId":"sess-a","status":"unavailable"}),
            1_000_000,
        );
        assert!(!f.context.needs_restart);
        f.context.observe_snapshot(
            &json!({"sessionId":"sess-a","status":"error","turnId":"turn-original"}),
            1_000_000,
        );
        assert!(f.context.needs_restart);
        assert_eq!(f.context.next_retry, 1_060_000);
    }
    #[test]
    fn pending_input_or_manual_pause_is_never_retried() {
        for status in ["waiting", "paused"] {
            let mut f = Fixture::new();
            f.failure();
            f.context.needs_restart = false;
            f.context.pending.get_mut("sess-a").unwrap().submitted_at = Some(1);
            f.context
                .observe_snapshot(&json!({"sessionId":"sess-a","status":status}), 1_000_000);
            assert!(f.context.pending.is_empty());
            assert!(!f.context.needs_restart);
            assert_eq!(f.context.state.recovered, 0);
        }
    }
    #[test]
    fn committed_launch_survives_failure_and_can_finish_after_cancellation() {
        let mut f = Fixture::new();
        f.context.launch_pending = true;
        assert!(!f.context.enabled());
        assert!(f
            .context
            .finish_launch(|| Err("launch failure".into()))
            .is_err());
        assert!(f.context.launch_pending);
        assert!(f.context.instance.is_none());
        f.context
            .finish_launch(|| {
                Ok(Instance {
                    pid: 42,
                    created_ms: 1,
                    executable: "fixture.exe".into(),
                    ports: vec![],
                })
            })
            .unwrap();
        assert!(!f.context.launch_pending);
        assert_eq!(f.context.instance.as_ref().unwrap().pid, 42);
        assert!(!f.context.enabled());
        assert!(f.context.pending.is_empty());
    }
    #[test]
    fn cancellation_loop_retains_only_launch_repair_until_success() {
        let mut f = Fixture::new();
        f.context.launch_pending = true;
        assert!(!f
            .context
            .finish_cancelled_attempt(Err("children still exiting".into())));
        assert!(f.context.launch_pending);
        assert!(f.context.next_retry > chrono::Utc::now().timestamp_millis());
        assert!(!f.context.enabled());
        assert!(f.context.pending.is_empty());
        assert!(f.context.finish_cancelled_attempt(Ok(())));
        assert!(!f.context.launch_pending);
        assert!(!f.context.enabled());
    }
}
