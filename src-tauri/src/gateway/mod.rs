//! 本地聚合网关：axum 入站 + 账号池调度 + 上游状态机 + 流式透传。
//!
//! 生命周期：随 GUI 进程启动（托盘常驻），单实例绑定 127.0.0.1:<port>；
//! 客户端凭 gateway_key 访问（fail-closed）。上游调用在 spawn_blocking 里跑
//! 同步 ureq，流式响应经 mpsc 泵进 axum Body（客户端断开即停泵释放槽位）。

pub mod affinity;
pub mod captcha;
pub mod error;
pub mod openai;
pub mod pool;
pub mod refresh;
pub mod transform;

use crate::store::{self, Paths};
use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use pool::{now_ms, Pool, PoolStatus, MAX_ACCOUNT_ATTEMPTS};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

const DEFAULT_PORT: u16 = 8718;
const BUSY_WINDOW_MS: i64 = 60_000;
const BUSY_THRESHOLD: usize = 5;
const BUSY_HOLD_MS: i64 = 180_000;
const EDGE_HOLD_MS: i64 = 180_000;
const MODEL_COOLDOWN_MS: i64 = 1800_000;
const MAX_STREAM_BODY: u64 = 64 * 1024 * 1024;
const REQLOG_MAX: usize = 500;

// ── 配置与状态 ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct GatewayConfig {
    pub enabled: bool,
    pub port: u16,
    pub key: String,
    pub solver_python: Option<String>,
    pub solver_script: Option<String>,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        GatewayConfig {
            enabled: false,
            port: DEFAULT_PORT,
            key: String::new(),
            solver_python: None,
            solver_script: None,
        }
    }
}

pub struct GatewayState {
    cfg: RwLock<GatewayConfig>,
    pub pool: Arc<Pool>,
    pub affinity: affinity::Affinity,
    pub paths: Paths,
    solver: RwLock<Option<Arc<captcha::CaptchaSolver>>>,
    captcha_cfg: Mutex<Option<(crate::claim::CaptchaConfig, i64)>>, // (config, fetched_at)
    busy_hits: Mutex<Vec<i64>>,
    busy_until: AtomicI64,
    edge_until: AtomicI64,
    model_cooldown: Mutex<HashMap<String, i64>>,
    reqlog: Mutex<VecDeque<Value>>,
}

impl GatewayState {
    fn new(cfg: GatewayConfig, paths: Paths) -> GatewayState {
        let solver = captcha::SolverConfig::resolve(cfg.solver_python.as_deref(), cfg.solver_script.as_deref())
            .map(captcha::CaptchaSolver::new)
            .map(Arc::new);
        GatewayState {
            cfg: RwLock::new(cfg),
            pool: Arc::new(Pool::new(paths.store_dir().join("gateway-state.json"))),
            affinity: affinity::Affinity::new(),
            paths,
            solver: RwLock::new(solver),
            captcha_cfg: Mutex::new(None),
            busy_hits: Mutex::new(Vec::new()),
            busy_until: AtomicI64::new(0),
            edge_until: AtomicI64::new(0),
            model_cooldown: Mutex::new(HashMap::new()),
            reqlog: Mutex::new(VecDeque::new()),
        }
    }

    fn config(&self) -> GatewayConfig {
        self.cfg.read().map(|c| c.clone()).unwrap_or_default()
    }

    fn gateway_key(&self) -> String {
        self.cfg.read().map(|c| c.key.clone()).unwrap_or_default()
    }

    fn log_req(&self, mut entry: Value) {
        if let Ok(mut g) = self.reqlog.lock() {
            if g.len() >= REQLOG_MAX {
                g.pop_front();
            }
            entry["ts"] = json!(chrono::Local::now().format("%m-%d %H:%M:%S").to_string());
            g.push_back(entry);
        }
    }

    fn note_busy(&self) {
        let now = now_ms();
        let trip = self
            .busy_hits
            .lock()
            .map(|mut g| {
                g.retain(|t| now - *t < BUSY_WINDOW_MS);
                g.push(now);
                g.len() >= BUSY_THRESHOLD
            })
            .unwrap_or(false);
        if trip {
            self.busy_until.store(now + BUSY_HOLD_MS, Ordering::Relaxed);
            eprintln!("[zsw-gw] busy 断路器：60s 窗口并发命中 ≥{BUSY_THRESHOLD}，全池暂停 180s（防 405 封 IP）");
        }
    }

    fn cached_captcha_param(&self) -> Option<String> {
        let guard = self.solver.read().ok()?;
        guard.as_ref().and_then(|s| s.peek())
    }

    fn recent_requests(&self, limit: usize) -> Vec<Value> {
        self.reqlog
            .lock()
            .map(|g| g.iter().rev().take(limit).cloned().collect())
            .unwrap_or_default()
    }
}

// ── 生命周期 ─────────────────────────────────────────────────────────────────

static HANDLE: OnceLock<Mutex<Option<GatewayHandle>>> = OnceLock::new();
static LIVE_STATE: OnceLock<Arc<GatewayState>> = OnceLock::new();

pub struct GatewayHandle {
    port: u16,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl GatewayHandle {
    pub fn port(&self) -> u16 {
        self.port
    }
}

/// 启动网关（已运行则先停旧的）。绑定失败返回 Err。
pub fn start(cfg: GatewayConfig, paths: Paths) -> Result<u16, String> {
    stop();
    if !cfg.enabled {
        return Ok(0);
    }
    let state = Arc::new(GatewayState::new(cfg.clone(), paths));
    let _ = LIVE_STATE.set(state.clone());
    // 同步 bind 提前暴露端口冲突；std→tokio 的转换必须在 runtime 上下文内做
    //（from_std 要注册 IO driver），因此放进服务线程的 block_on 里。
    let std_listener = std::net::TcpListener::bind(("127.0.0.1", cfg.port))
        .map_err(|e| format!("绑定 127.0.0.1:{} 失败: {e}", cfg.port))?;
    let port = std_listener.local_addr().map(|a| a.port()).unwrap_or(cfg.port);
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let st = state.clone();
    std::thread::Builder::new()
        .name("zsw-gw-serve".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("gateway tokio runtime");
            rt.block_on(async move {
                std_listener.set_nonblocking(true).ok();
                let Ok(tokio_listener) = tokio::net::TcpListener::from_std(std_listener) else {
                    eprintln!("[zsw-gw] listener 转换失败，网关退出");
                    return;
                };
                let app = build_router(st);
                axum::serve(tokio_listener, app.into_make_service_with_connect_info::<SocketAddr>())
                    .with_graceful_shutdown(async {
                        let _ = rx.await;
                    })
                    .await
                    .ok();
            });
        })
        .map_err(|e| format!("网关线程启动失败: {e}"))?;
    // 启动额度刷新循环
    refresh::spawn_loop(state.paths.clone(), state.pool.clone());
    let handle_cell = HANDLE.get_or_init(|| Mutex::new(None));
    if let Ok(mut g) = handle_cell.lock() {
        *g = Some(GatewayHandle { port, shutdown: Some(tx) });
    }
    eprintln!("[zsw-gw] 网关已启动 http://127.0.0.1:{port}（key {}）", if cfg.key.is_empty() { "未设置=拒绝一切请求" } else { "已设置" });
    Ok(port)
}

pub fn stop() {
    if let Some(cell) = HANDLE.get() {
        if let Ok(mut g) = cell.lock() {
            if let Some(h) = g.take() {
                if let Some(tx) = h.shutdown {
                    let _ = tx.send(());
                }
                eprintln!("[zsw-gw] 网关已停止（原端口 {}）", h.port);
            }
        }
    }
}

pub fn running_port() -> Option<u16> {
    HANDLE.get().and_then(|c| c.lock().ok()).and_then(|g| g.as_ref().map(|h| h.port()))
}

/// 设置变更后按新配置重启（enabled=false 即停）
pub fn apply_config(cfg: GatewayConfig, paths: Paths) -> Result<u16, String> {
    start(cfg, paths)
}

// ── 路由 ─────────────────────────────────────────────────────────────────────

fn build_router(state: Arc<GatewayState>) -> Router {
    Router::new()
        .route("/", get(root_page))
        .route("/health", get(health))
        .route("/v1/models", get(list_models))
        .route("/v1/messages", post(messages))
        .route("/v1/chat/completions", post(chat_completions))
        .with_state(state)
}

async fn root_page(State(st): State<Arc<GatewayState>>, headers: HeaderMap) -> Response {
    if !authorized(&st, &headers) {
        // 未鉴权只给极简说明，不泄露池信息
        return (
            StatusCode::OK,
            [("content-type", "text/html; charset=utf-8")],
            "<!doctype html><meta charset=\"utf-8\"><title>zcode-switch gateway</title><body style=\"font-family:sans-serif;background:#0a0a0c;color:#ddd;display:grid;place-items:center;height:100vh;margin:0\"><div style=\"text-align:center\"><h1>Z·SWITCH 聚合网关</h1><p>本服务是纯 API 网关（/v1/messages · /v1/chat/completions · /v1/models），无网页后台。</p><p>管理界面在 zcode-switch 应用里：设置页配置开关/端口/密钥，主窗查看账号池。</p><p>客户端接入：Authorization: Bearer &lt;访问密钥&gt;，密钥在 zcode-switch 设置页生成。</p></div></body>",
        ).into_response();
    }
    let st_ = st.clone();
    let _ = st_;
    health(State(st)).await
}

async fn health(State(st): State<Arc<GatewayState>>) -> Response {
    let n = st.pool.view(&account_ids(&st.paths), &HashMap::new()).len();
    (StatusCode::OK, axum::Json(json!({ "ok": true, "accounts": n, "port": running_port() }))).into_response()
}

async fn list_models(
    State(st): State<Arc<GatewayState>>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&st, &headers) {
        return auth_fail();
    }
    let data: Vec<Value> = transform::AVAILABLE_MODELS
        .iter()
        .map(|m| json!({ "type": "model", "id": m, "display_name": m }))
        .collect();
    (StatusCode::OK, axum::Json(json!({ "data": data, "has_more": false }))).into_response()
}

async fn messages(
    State(st): State<Arc<GatewayState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    handle_endpoint(st, addr, headers, body, Endpoint::Messages).await
}

async fn chat_completions(
    State(st): State<Arc<GatewayState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    handle_endpoint(st, addr, headers, body, Endpoint::OpenAIChat).await
}

#[derive(Clone, Copy, PartialEq)]
enum Endpoint {
    Messages,
    OpenAIChat,
}

impl Endpoint {
    fn path(&self) -> &'static str {
        match self {
            Endpoint::Messages => "/v1/messages",
            Endpoint::OpenAIChat => "/v1/chat/completions",
        }
    }
}

// ── 鉴权 ─────────────────────────────────────────────────────────────────────

/// 常数时间比较（不引入新依赖）
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn authorized(st: &GatewayState, headers: &HeaderMap) -> bool {
    let key = st.gateway_key();
    if key.is_empty() {
        return false; // fail-closed
    }
    let presented = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            headers
                .get("x-api-key")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|s| !s.is_empty())
        });
    presented.is_some_and(|p| ct_eq(p, &key))
}

fn auth_fail() -> Response {
    anthropic_error(StatusCode::FORBIDDEN, "authentication_error", "无效的访问密钥（gateway key）")
}

fn anthropic_error(status: StatusCode, etype: &str, msg: &str) -> Response {
    (
        status,
        axum::Json(json!({ "type": "error", "error": { "type": etype, "message": msg } })),
    )
        .into_response()
}

fn openai_error(status: StatusCode, msg: &str) -> Response {
    (
        status,
        axum::Json(json!({ "error": { "message": msg, "type": "server_error" } })),
    )
        .into_response()
}

// ── 端点主流程 ───────────────────────────────────────────────────────────────

async fn handle_endpoint(
    st: Arc<GatewayState>,
    addr: SocketAddr,
    headers: HeaderMap,
    raw: axum::body::Bytes,
    ep: Endpoint,
) -> Response {
    if !authorized(&st, &headers) {
        return auth_fail();
    }
    // busy / edge 断路器：全池暂停期直接拒绝
    let now = now_ms();
    for (until, reason) in [
        (st.busy_until.load(Ordering::Relaxed), "并发断路器全池暂停中"),
        (st.edge_until.load(Ordering::Relaxed), "上游边缘封禁暂停中"),
    ] {
        if until > now {
            let retry = ((until - now) / 1000).max(1).to_string();
            let mut resp = anthropic_error(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limit_error",
                &format!("{reason}，请稍后重试"),
            );
            resp.headers_mut().insert("retry-after", retry.parse().unwrap());
            return resp;
        }
    }

    // OpenAI 入口先翻译成 Anthropic body；/v1/messages 直接用
    let mut body: Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(e) => {
            return bad_request(&ep, &format!("请求体不是合法 JSON: {e}"));
        }
    };
    let client_model = body.get("model").and_then(Value::as_str).unwrap_or("").to_string();
    let mut openai_stream = false;
    if ep == Endpoint::OpenAIChat {
        match openai::request_to_anthropic(&body) {
            Ok(v) => {
                openai_stream = v.get("stream").and_then(Value::as_bool) == Some(true);
                body = v;
            }
            Err(e) => return bad_request(&ep, &e),
        }
    }
    let wants_stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false) || openai_stream;
    transform::normalize_body(&mut body);
    let model = body.get("model").and_then(Value::as_str).unwrap_or("").to_string();

    let req_id = uuid::Uuid::new_v4().simple().to_string();
    let t0 = std::time::Instant::now();
    st.log_req(json!({ "req_id": req_id, "endpoint": ep.path(), "model": model, "stream": wants_stream, "preview": preview_of(&body) }));

    let ip = addr.ip().to_string();
    let hdr_list: Vec<(String, String)> = headers
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.as_str().to_string(), s.to_string())))
        .collect();
    let ctx = st.affinity.extract("zai", &hdr_list, &ip, &body);
    let mut pinned = st.affinity.pinned_account(&ctx, now_s());

    let ids = account_ids(&st.paths);
    let mut tried: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut attempts = 0usize;
    let mut in_place_retries = 0u32;
    let mut concurrency_streak = 0u32;
    let mut last_err_msg = String::from("无可用账号");

    loop {
        if attempts >= MAX_ACCOUNT_ATTEMPTS {
            break;
        }
        let Some(acc_id) = pinned
            .take()
            .filter(|p| !tried.contains(p))
            .or_else(|| st.pool.select(&ids, &tried, None))
        else {
            break;
        };
        // (账号,模型) 冷却跳过不计入预算
        if model_cooled(&st, &acc_id, &model) {
            tried.insert(acc_id);
            continue;
        }
        // 槽位满载跳过不计入预算
        if !st.pool.acquire_slot(&acc_id) {
            tried.insert(acc_id);
            continue;
        }
        attempts += 1;
        tried.insert(acc_id.clone());
        st.log_req(json!({ "req_id": req_id, "event": "try", "account": acc_id, "attempt": attempts }));

        match try_account(&st, &acc_id, &body, &model, wants_stream, &ctx).await {
            TryOutcome::Stream { status, content_type, reader, account } => {
                let t_first = t0.elapsed().as_millis() as i64;
                st.log_req(json!({ "req_id": req_id, "event": "ok", "account": account, "t_first": t_first, "stream": true }));
                mark_success(&st, &account);
                st.affinity.bind(&ctx, &account);
                return stream_response(ep, &client_model, status, content_type, reader, st.clone(), account);
            }
            TryOutcome::Full { status, content_type, body: resp_body, account } => {
                let t_total = t0.elapsed().as_millis() as i64;
                st.log_req(json!({ "req_id": req_id, "event": "ok", "account": account, "t_total": t_total, "stream": false }));
                mark_success(&st, &account);
                st.affinity.bind(&ctx, &account);
                st.pool.release_slot(&account);
                return full_response(ep, &client_model, status, content_type, resp_body);
            }
            TryOutcome::RetryAfterWait { secs } => {
                st.pool.release_slot(&acc_id);
                in_place_retries += 1;
                if in_place_retries > error::RETRY_429_TIMES {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(secs.min(120))).await;
                pinned = Some(acc_id); // 原地重试同号
                continue;
            }
            TryOutcome::ConcurrencyWait { wait } => {
                st.pool.release_slot(&acc_id);
                concurrency_streak += 1;
                st.note_busy();
                if concurrency_streak >= error::CONCURRENCY_GIVEUP_AFTER {
                    concurrency_streak = 0;
                    continue; // 止损换号
                }
                tokio::time::sleep(Duration::from_secs(wait.min(error::CONCURRENCY_WAIT_MAX))).await;
                pinned = Some(acc_id);
                continue;
            }
            TryOutcome::NextAccount { wait_secs } => {
                st.pool.release_slot(&acc_id);
                if let Some(w) = wait_secs {
                    tokio::time::sleep(Duration::from_secs(w.min(30))).await;
                }
                continue;
            }
            TryOutcome::ClientPass { status, content_type, body: resp_body, account } => {
                st.pool.release_slot(&account);
                st.log_req(json!({ "req_id": req_id, "event": "pass", "account": account, "status": status }));
                return full_response(ep, &client_model, status, content_type, resp_body);
            }
            TryOutcome::Fatal(msg) => {
                st.pool.release_slot(&acc_id);
                last_err_msg = msg;
                break;
            }
        }
    }

    // 全部失败
    let t_total = t0.elapsed().as_millis() as i64;
    st.log_req(json!({ "req_id": req_id, "event": "fail", "attempts": attempts, "t_total": t_total, "error": last_err_msg }));
    let mut resp = anthropic_error(StatusCode::TOO_MANY_REQUESTS, "rate_limit_error", &last_err_msg);
    resp.headers_mut().insert("retry-after", "5".parse().unwrap());
    resp
}

fn bad_request(ep: &Endpoint, msg: &str) -> Response {
    match ep {
        Endpoint::Messages => anthropic_error(StatusCode::BAD_REQUEST, "invalid_request_error", msg),
        Endpoint::OpenAIChat => openai_error(StatusCode::BAD_REQUEST, msg),
    }
}

fn preview_of(body: &Value) -> String {
    transform::extract_first_user_text(body)
        .map(|s| transform::truncate_chars(&s, 60))
        .unwrap_or_default()
}

fn account_ids(paths: &Paths) -> Vec<String> {
    store::list_accounts(paths)
        .map(|accs| accs.iter().map(|a| a.id.clone()).collect())
        .unwrap_or_default()
}

fn now_s() -> i64 {
    now_ms() / 1000
}

fn model_cooled(st: &GatewayState, acc: &str, model: &str) -> bool {
    let Ok(g) = st.model_cooldown.lock() else { return false };
    g.get(&format!("{acc}\n{model}")).is_some_and(|until| *until > now_ms())
}

fn mark_success(st: &Arc<GatewayState>, acc: &str) {
    let now = now_ms();
    let decay = |strikes: u32, last: i64| -> u32 {
        if last > 0 && now - last > error::RISK_STRIKE_DECAY_MS {
            0
        } else {
            strikes
        }
    };
    st.pool.with_state(acc, |s| {
        s.use_count += 1;
        s.last_used_at = now;
        let cur = s.status_or_active();
        // 守卫：INVALID/DISABLED/未到期冷却不被成功洗回 ACTIVE
        if !matches!(cur, PoolStatus::Invalid | PoolStatus::Disabled) {
            if s.risk_strikes > 0 {
                s.risk_strikes = decay(s.risk_strikes, s.last_risk_at);
                if s.risk_strikes == 0 {
                    s.last_risk_at = 0;
                }
            }
            if s.risk_strikes == 0 && cur == PoolStatus::Cooling && s.cooling_until <= now {
                s.status = Some(PoolStatus::Active);
                s.cooling_until = 0;
            }
            s.last_error.clear();
        }
    });
    st.pool.persist();
    // 成功后按需刷新额度（去抖 60s；独立线程不阻塞响应）
    let paths = st.paths.clone();
    let pool = st.pool.clone();
    let id = acc.to_string();
    std::thread::Builder::new()
        .name("zsw-gw-ondemand".into())
        .spawn(move || refresh::maybe_refresh_on_success(&paths, &pool, &id))
        .ok();
}

// ── 上游通道 ─────────────────────────────────────────────────────────────────

struct Rung {
    url: String,
    is_plan: bool,
    token: String,
    bigmodel: bool,
}

fn resolve_rungs(paths: &Paths, acc: &store::Account) -> Vec<Rung> {
    let home = &paths.home;
    let secret = crate::zcrypto::default_secret(home);
    let mut rungs = vec![];
    // 通道 1：Plan JWT（未过期）
    if let Some(jwt) = crate::store::cred_plain_opt(&acc.credentials, "zcodejwttoken", home) {
        let expired = crate::zcrypto::jwt_exp_ms(&jwt)
            .map(|exp| exp < now_ms() + 30_000)
            .unwrap_or(false);
        if !jwt.is_empty() && !expired {
            rungs.push(Rung {
                url: transform::PLAN_MESSAGES_URL.to_string(),
                is_plan: true,
                token: jwt,
                bigmodel: false,
            });
        }
    }
    // 通道 2：API Key 回退（凭据里 account-provider:*:api-key）
    if let Some(obj) = acc.credentials.as_object() {
        for (k, v) in obj {
            let Some(raw) = v.as_str() else { continue };
            if !(k.starts_with("account-provider:") && k.ends_with(":api-key")) {
                continue;
            }
            let key = crate::zcrypto::decrypt_with_secret(raw, &secret).unwrap_or_else(|_| raw.to_string());
            if key.len() <= 20 {
                continue;
            }
            let bigmodel = k.contains("bigmodel") || k.contains("coding-plan");
            rungs.push(Rung {
                url: if bigmodel { transform::BIGMODEL_MESSAGES_URL.to_string() } else { transform::FALLBACK_MESSAGES_URL.to_string() },
                is_plan: false,
                token: key,
                bigmodel,
            });
        }
    }
    rungs
}

fn upstream_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .timeout_read(Duration::from_secs(600))
        .timeout(Duration::from_secs(3600))
        .build()
}

enum TryOutcome {
    /// 成功流式响应（reader 交由泵线程；槽位由泵结束时释放）
    Stream {
        status: u16,
        content_type: String,
        reader: Box<dyn Read + Send>,
        account: String,
    },
    /// 成功非流式（body 已读净并解析）
    Full { status: u16, content_type: String, body: Value, account: String },
    /// 其它 429：等待 secs 后原地重试同号
    RetryAfterWait { secs: u64 },
    /// 并发族：短退避后原地重试
    ConcurrencyWait { wait: u64 },
    /// 换号（可选先等 wait_secs）
    NextAccount { wait_secs: Option<u64> },
    /// 4xx 原样回传客户端
    ClientPass { status: u16, content_type: String, body: Value, account: String },
    /// 请求构造层错误（如无可用通道），终止
    Fatal(String),
}

async fn try_account(
    st: &Arc<GatewayState>,
    acc_id: &str,
    body: &Value,
    model: &str,
    wants_stream: bool,
    ctx: &affinity::ContextInfo,
) -> TryOutcome {
    let st = st.clone();
    let acc_id = acc_id.to_string();
    let mut body = body.clone();
    let model = model.to_string();
    let ctx = ctx.clone();
    tokio::task::spawn_blocking(move || try_account_blocking(&st, &acc_id, &mut body, &model, wants_stream, &ctx))
        .await
        .unwrap_or_else(|e| TryOutcome::Fatal(format!("上游任务崩溃: {e}")))
}

#[allow(clippy::too_many_arguments)]
fn try_account_blocking(
    st: &Arc<GatewayState>,
    acc_id: &str,
    body: &mut Value,
    model: &str,
    wants_stream: bool,
    ctx: &affinity::ContextInfo,
) -> TryOutcome {
    let Ok(acc) = store::load_account(&st.paths, acc_id) else {
        return TryOutcome::Fatal("账号加载失败".into());
    };
    let mid = store::ensure_virtual_device_mid(&st.paths, acc_id).ok();
    let rungs = resolve_rungs(&st.paths, &acc);
    if rungs.is_empty() {
        return TryOutcome::Fatal(format!("账号 {acc_id} 无可用对话通道（JWT 过期且无 API Key）"));
    }
    // 会话/轮次 id（变换前计算，确定性；重试/换号重复调用结果一致）
    let session_id = transform::conversation_session_id(body, acc_id)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let query_id = transform::conversation_query_id(body, Some(&session_id));

    let agent = upstream_agent();
    for rung in &rungs {
        if rung.is_plan {
            transform::transform_plan_body(body, mid.as_deref(), Some(&session_id));
        }
        let payload = body.to_string();
        let base_headers =
            build_headers(st, rung, &session_id, query_id.as_deref(), mid.as_deref());

        // Plan 通道单号最多 3 次验证码重试（解出后 45s TTL 内立即发）
        let mut captcha_tries = 0u32;
        let mut extra_headers: Vec<(String, String)> = Vec::new();
        loop {
            let mut headers = base_headers.clone();
            headers.extend(extra_headers.drain(..));
            match send_upstream(&agent, rung, &headers, &payload) {
                SendResult::Ok(resp) => {
                    let status = resp.status();
                    let content_type = content_type_str(&resp);
                    if (200..300).contains(&status) {
                        return finish_ok(resp, status, content_type, acc_id, wants_stream);
                    }
                    let body_text = read_bounded(resp, 4 * 1024 * 1024);
                    let parsed: Value =
                        serde_json::from_str(&body_text).unwrap_or(Value::String(body_text));
                    return handle_verdict(
                        st,
                        acc_id,
                        model,
                        error::classify(status, Some(&parsed), &[]),
                        Some(parsed),
                        content_type,
                    );
                }
                SendResult::Status(status, resp) => {
                    let content_type = content_type_str(&resp);
                    let body_text = read_bounded(resp, 4 * 1024 * 1024);
                    let parsed: Value =
                        serde_json::from_str(&body_text).unwrap_or(Value::String(body_text));
                    let verdict = error::classify(status, Some(&parsed), &[]);
                    if verdict == error::Verdict::Captcha && rung.is_plan && captcha_tries < 3 {
                        captcha_tries += 1;
                        match solve_captcha(st) {
                            Ok(param) => {
                                extra_headers.push((captcha::ENV_VERIFY_PARAM.into(), param));
                                if let Some(cc) = captcha_config_cached(st) {
                                    if !cc.region.is_empty() {
                                        extra_headers
                                            .push((captcha::ENV_VERIFY_REGION.into(), cc.region));
                                    }
                                }
                                continue; // 带 param 重发（TTL 内）
                            }
                            Err(e) => {
                                eprintln!("[zsw-gw] 验证码求解失败（降级冷却换号）: {e}");
                            }
                        }
                        cooldown(st, acc_id, error::CAPTCHA_COOLDOWN, "验证码挑战且求解不可用");
                        return TryOutcome::NextAccount { wait_secs: None };
                    }
                    return handle_verdict(st, acc_id, model, verdict, Some(parsed), content_type);
                }
                SendResult::Transport(e) => {
                    // 连接层失败：短冷却换号（出口瞬态，非账号故障）
                    cooldown(st, acc_id, error::CONNECT_FAIL_COOLDOWN, &format!("连接失败: {e}"));
                    return TryOutcome::NextAccount { wait_secs: None };
                }
            }
        }
    }
    TryOutcome::NextAccount { wait_secs: None }
}

fn build_headers(
    st: &Arc<GatewayState>,
    rung: &Rung,
    session_id: &str,
    query_id: Option<&str>,
    mid: Option<&str>,
) -> Vec<(String, String)> {
    let mut h: Vec<(String, String)> = vec![
        ("content-type".into(), "application/json".into()),
        ("anthropic-version".into(), transform::ANTHROPIC_VERSION.into()),
    ];
    if rung.is_plan {
        h.push(("Authorization".into(), format!("Bearer {}", rung.token)));
        h.extend(transform::plan_identity_headers(mid));
        h.extend(transform::trace_headers(session_id, query_id));
        // 已解出的验证码 param 直接带上（45s TTL 内）
        if let Some(param) = st.cached_captcha_param() {
            h.push((captcha::ENV_VERIFY_PARAM.into(), param));
            if let Some(cc) = captcha_config_cached(st) {
                if !cc.region.is_empty() {
                    h.push((captcha::ENV_VERIFY_REGION.into(), cc.region));
                }
            }
        }
    } else {
        h.push(("x-api-key".into(), rung.token.clone()));
        h.push(("User-Agent".into(), format!("ZCode/{}", transform::CHAT_APP_VERSION)));
        h.push(("X-ZCode-App-Version".into(), transform::CHAT_APP_VERSION.into()));
        h.push(("X-ZCode-Agent".into(), "glm".into()));
        h.push(("HTTP-Referer".into(), transform::HTTP_REFERER.into()));
    }
    h
}

fn captcha_config_cached(st: &Arc<GatewayState>) -> Option<crate::claim::CaptchaConfig> {
    let now = now_ms();
    if let Ok(mut g) = st.captcha_cfg.lock() {
        if let Some((cc, at)) = g.as_ref() {
            if now - *at < 3600_000 {
                return Some(cc.clone());
            }
        }
    }
    let cc = crate::claim::fetch_captcha_config().ok()?;
    if let Ok(mut g) = st.captcha_cfg.lock() {
        *g = Some((cc.clone(), now));
    }
    Some(cc)
}

fn solve_captcha(st: &Arc<GatewayState>) -> Result<String, String> {
    // Arc 克隆后立刻放锁：90s 求解期间不阻塞 invalidate/peek
    let solver = {
        let guard = st.solver.read().map_err(|_| "求解器锁不可用")?;
        guard.as_ref().cloned()
    };
    let Some(solver) = solver else {
        return Err("求解器未配置".into());
    };
    let cc = captcha_config_cached(st).ok_or("验证码配置获取失败")?;
    if !cc.enabled {
        return Err("上游未启用验证码".into());
    }
    solver.solve(&cc.scene_id, &cc.region, &cc.prefix)
}

/// ureq 的 content_type() 直接返回 &str（空串表示无），归一为可用值
fn content_type_str(resp: &ureq::Response) -> String {
    let c = resp.content_type();
    if c.is_empty() { "application/json".into() } else { c.to_string() }
}

enum SendResult {
    Ok(ureq::Response),
    Status(u16, ureq::Response),
    Transport(String),
}

fn send_upstream(agent: &ureq::Agent, rung: &Rung, headers: &[(String, String)], payload: &str) -> SendResult {
    let mut req = agent.post(&rung.url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    match req.send_string(payload) {
        Ok(resp) => SendResult::Ok(resp),
        Err(ureq::Error::Status(code, resp)) => SendResult::Status(code, resp),
        Err(e) => SendResult::Transport(e.to_string()),
    }
}

fn read_bounded(resp: ureq::Response, cap: u64) -> String {
    let mut reader = resp.into_reader().take(cap);
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).ok();
    String::from_utf8_lossy(&buf).to_string()
}

/// 2xx 出口：流式返回 reader（泵线程接管），非流式读净解析
fn finish_ok(
    resp: ureq::Response,
    status: u16,
    content_type: String,
    acc_id: &str,
    _wants_stream: bool,
) -> TryOutcome {
    let is_sse = content_type.contains("text/event-stream");
    if is_sse {
        return TryOutcome::Stream {
            status,
            content_type,
            reader: Box::new(resp.into_reader()),
            account: acc_id.to_string(),
        };
    }
    let text = read_bounded(resp, MAX_STREAM_BODY);
    let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    TryOutcome::Full { status, content_type, body: parsed, account: acc_id.to_string() }
}

/// 非成功判定 → 池状态落地 + 调度决策
fn handle_verdict(
    st: &Arc<GatewayState>,
    acc_id: &str,
    model: &str,
    verdict: error::Verdict,
    body: Option<Value>,
    content_type: String,
) -> TryOutcome {
    use error::Verdict as V;
    let now = now_ms();
    match verdict {
        V::Ok => TryOutcome::NextAccount { wait_secs: None }, // 不应到达
        V::Captcha => {
            // 调用方已处理重试；兜底冷却换号
            cooldown(st, acc_id, error::CAPTCHA_COOLDOWN, "验证码挑战");
            TryOutcome::NextAccount { wait_secs: None }
        }
        V::Exhausted => {
            st.pool.with_state(acc_id, |s| {
                s.status = Some(PoolStatus::Exhausted);
                s.fail_count += 1;
                s.last_error = format!("额度耗尽（{model}）");
            });
            if !model.is_empty() {
                if let Ok(mut g) = st.model_cooldown.lock() {
                    g.insert(format!("{acc_id}\n{model}"), now + MODEL_COOLDOWN_MS);
                }
            }
            st.pool.persist();
            TryOutcome::NextAccount { wait_secs: None }
        }
        V::RateLimited => {
            cooldown(st, acc_id, error::RATE_LIMIT_COOLDOWN, "上游真限流（1302/1303）");
            TryOutcome::NextAccount { wait_secs: None }
        }
        V::Concurrency => {
            // 不冷却账号；调度器负责短退避 + busy 断路器
            st.pool.with_state(acc_id, |s| {
                s.fail_count += 1;
                s.last_error = "并发争用".into();
            });
            TryOutcome::ConcurrencyWait { wait: error::CONCURRENCY_WAIT_BASE }
        }
        V::Rate429 { retry_after } => {
            let secs = retry_after.unwrap_or(error::RATE_LIMIT_COOLDOWN as u64);
            TryOutcome::RetryAfterWait { secs }
        }
        V::Risk => {
            let strikes = st
                .pool
                .with_state(acc_id, |s| {
                    s.risk_strikes += 1;
                    s.last_risk_at = now;
                    s.last_error = "上游风控（3012/405）".into();
                    s.risk_strikes
                })
                .unwrap_or(1);
            if strikes >= error::RISK_BAN_STRIKES {
                st.pool.with_state(acc_id, |s| s.status = Some(PoolStatus::Disabled));
            } else {
                cooldown(st, acc_id, error::risk_cooldown_secs(strikes), "上游风控");
            }
            st.pool.persist();
            eprintln!(
                "[zsw-gw] 账号 {acc_id} 风控第 {strikes} 次，退避 {}s",
                error::risk_cooldown_secs(strikes)
            );
            TryOutcome::NextAccount { wait_secs: None }
        }
        V::EdgeBlock => {
            st.edge_until.store(now + EDGE_HOLD_MS, Ordering::Relaxed);
            eprintln!("[zsw-gw] 边缘封禁（405 封 IP 页面），全进程暂停 plan 渠道 180s（账号不惩罚）");
            TryOutcome::NextAccount { wait_secs: None }
        }
        V::Auth => {
            st.pool.with_state(acc_id, |s| {
                s.status = Some(PoolStatus::Invalid);
                s.fail_count += 1;
                s.last_error = "凭据失效（401/403）".into();
            });
            st.pool.persist();
            TryOutcome::NextAccount { wait_secs: None }
        }
        V::ServerErr { status } => {
            st.pool.with_state(acc_id, |s| {
                s.fail_count += 1;
                s.last_error = format!("上游 {status}");
            });
            TryOutcome::NextAccount { wait_secs: Some(error::RETRY_5XX_WAIT) }
        }
        V::ConnectFail => {
            cooldown(st, acc_id, error::CONNECT_FAIL_COOLDOWN, "连接失败");
            TryOutcome::NextAccount { wait_secs: None }
        }
        V::ClientError { status } => {
            st.pool.with_state(acc_id, |s| {
                s.fail_count += 1;
            });
            TryOutcome::ClientPass {
                status,
                content_type,
                body: body.unwrap_or(Value::Null),
                account: acc_id.to_string(),
            }
        }
    }
}

fn cooldown(st: &Arc<GatewayState>, acc_id: &str, secs: i64, reason: &str) {
    st.pool.with_state(acc_id, |s| {
        let cur = s.status_or_active();
        if !matches!(cur, PoolStatus::Invalid | PoolStatus::Disabled | PoolStatus::Exhausted) {
            s.status = Some(PoolStatus::Cooling);
            s.cooling_until = now_ms() + secs * 1000;
        }
        s.last_error = reason.to_string();
    });
    st.pool.persist();
}

// ── 响应出口 ─────────────────────────────────────────────────────────────────

fn full_response(ep: Endpoint, client_model: &str, status: u16, content_type: String, body: Value) -> Response {
    let _ = content_type;
    let sc = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    // 200 承载业务错误的形态：body 里有非 0/200 code → 映射成 5xx
    let code = body.get("code").and_then(Value::as_i64);
    let eff_status = if (200u16..300).contains(&status) && code.is_some_and(|c| c != 0 && c != 200) {
        StatusCode::BAD_GATEWAY
    } else {
        sc
    };
    match ep {
        Endpoint::Messages => (eff_status, axum::Json(body)).into_response(),
        Endpoint::OpenAIChat => {
            if (200u16..300).contains(&status) && !code.is_some_and(|c| c != 0 && c != 200) {
                let converted = openai::response_to_openai(&body, client_model);
                (StatusCode::OK, axum::Json(converted)).into_response()
            } else {
                openai_error(eff_status, &format!("上游错误 {}", code.unwrap_or_default()))
            }
        }
    }
}

fn stream_response(
    ep: Endpoint,
    client_model: &str,
    status: u16,
    content_type: String,
    reader: Box<dyn Read + Send>,
    st: Arc<GatewayState>,
    account: String,
) -> Response {
    let sc = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    let is_openai = ep == Endpoint::OpenAIChat;
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(16);
    let client_model = client_model.to_string();
    // 泵线程：上游 reader → mpsc；结束/出错时释放槽位
    std::thread::Builder::new()
        .name("zsw-gw-pump".into())
        .spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 8192];
            let mut sse_carry = String::new();
            let mut conv = openai::StreamConverter::new(&client_model);
            let mut started = !is_openai;
            if is_openai {
                let _ = tx.blocking_send(Ok(openai::sse(&conv.start_event()).into_bytes()));
            }
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = if is_openai {
                            // SSE 行解析 → OpenAI chunk 转换
                            sse_carry.push_str(&String::from_utf8_lossy(&buf[..n]));
                            let mut out = Vec::new();
                            while let Some(pos) = sse_carry.find("\n\n") {
                                let block: String = sse_carry.drain(..pos + 2).collect();
                                for line in block.lines() {
                                    if let Some(data) = line.strip_prefix("data: ") {
                                        if data.trim() == "[DONE]" {
                                            continue;
                                        }
                                        if let Ok(evt) = serde_json::from_str::<Value>(data) {
                                            match conv.feed(&client_model, &evt) {
                                                Ok(chunks) => {
                                                    for c in chunks {
                                                        out.extend_from_slice(c.as_bytes());
                                                    }
                                                }
                                                Err(msg) => {
                                                    let err = openai::sse(&json!({
                                                        "id": conv.chunk_id,
                                                        "object": "chat.completion.chunk",
                                                        "created": conv.created,
                                                        "model": client_model,
                                                        "choices": [{ "index": 0, "delta": {}, "finish_reason": null }],
                                                        "error": { "message": msg, "type": "server_error" },
                                                    }));
                                                    out.extend_from_slice(err.as_bytes());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            out
                        } else {
                            buf[..n].to_vec()
                        };
                        if !chunk.is_empty() && tx.blocking_send(Ok(chunk)).is_err() {
                            break; // 客户端断开
                        }
                    }
                    Err(e) => {
                        let _ = tx.blocking_send(Err(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())));
                        break;
                    }
                }
            }
            if is_openai {
                let _ = tx.blocking_send(Ok(conv.done().as_bytes().to_vec()));
            }
            let _ = started;
            drop(tx); // 关闭流
            st.pool.release_slot(&account); // 槽位归还
            st.pool.persist();
        })
        .ok();
    let mut resp = Response::new(Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx)));
    *resp.status_mut() = sc;
    if let Ok(v) = content_type.parse() {
        resp.headers_mut().insert("content-type", v);
    }
    resp.headers_mut().insert("cache-control", "no-cache".parse().unwrap());
    resp
}

// ── 状态视图（Tauri 命令 / CLI 共用）────────────────────────────────────────

pub fn status_view(paths: &Paths) -> Value {
    // CLI 等独立进程没有本进程 HANDLE，用 TCP 探活兜底
    let port = running_port().or_else(|| {
        let want = crate::store::load_settings(paths).gateway_port();
        std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], want)),
            Duration::from_millis(500),
        )
        .is_ok()
        .then_some(want)
    });
    // 活网关直接用内存池（含在飞计数）；未运行时读持久化快照
    let (pool, recent): (Arc<Pool>, Vec<Value>) = match LIVE_STATE.get() {
        Some(st) => (st.pool.clone(), st.recent_requests(20)),
        None => (
            Arc::new(Pool::new(paths.store_dir().join("gateway-state.json"))),
            vec![],
        ),
    };
    let accounts: Vec<String> = store::list_accounts(paths)
        .map(|accs| accs.iter().map(|a| a.id.clone()).collect())
        .unwrap_or_default();
    let names: HashMap<String, String> = store::list_accounts(paths)
        .map(|accs| accs.iter().map(|a| (a.id.clone(), a.name.clone())).collect())
        .unwrap_or_default();
    json!({
        "ok": true,
        "running": port.is_some(),
        "port": port,
        "accounts": pool.view(&accounts, &names),
        "recent_requests": recent,
    })
}

/// 供 Tauri 命令组装 GatewayConfig
pub fn config_from_settings(settings: &store::Settings) -> GatewayConfig {
    GatewayConfig {
        enabled: settings.gateway_enabled(),
        port: settings.gateway_port(),
        key: settings.gateway_key.clone().unwrap_or_default(),
        solver_python: settings.captcha_solver_python.clone().filter(|s| !s.trim().is_empty()),
        solver_script: settings.captcha_solver_script.clone().filter(|s| !s.trim().is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_compare() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "ab"));
        assert!(ct_eq("", ""));
    }

    #[test]
    fn resolve_rungs_prefers_jwt_then_keys() {
        let dir = std::env::temp_dir().join(format!("gwrun-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths = Paths { home: dir.clone(), data_root: dir.clone() };
        let creds = json!({
            "zcodejwttoken": "x.y.z",
            "account-provider:builtin:bigmodel-coding-plan:api-key": "k",
        });
        let acc = store::Account {
            id: "t".into(),
            name: "t".into(),
            created_at: String::new(),
            updated_at: String::new(),
            hash: String::new(),
            credentials: creds,
            config: None,
            virtual_device_mid: None,
            virtual_arms_uid: None,
        };
        // JWT "x.y.z" 解不出 exp → 不判过期 → 通道 1 存在；api-key "k" 长度不足 → 不入梯
        let rungs = resolve_rungs(&paths, &acc);
        assert_eq!(rungs.len(), 1);
        assert!(rungs[0].is_plan);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn model_cooldown_key_shape() {
        let dir = std::env::temp_dir().join(format!("gwmc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = Arc::new(GatewayState::new(
            GatewayConfig::default(),
            Paths { home: dir.clone(), data_root: dir.clone() },
        ));
        st.model_cooldown
            .lock()
            .unwrap()
            .insert("acc1\nGLM-5.3".into(), now_ms() + 60_000);
        assert!(model_cooled(&st, "acc1", "GLM-5.3"));
        assert!(!model_cooled(&st, "acc1", "GLM-5.3-Flash"));
        assert!(!model_cooled(&st, "acc2", "GLM-5.3"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
