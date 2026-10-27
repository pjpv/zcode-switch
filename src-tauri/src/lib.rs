pub mod cipher;
pub mod cli;
pub mod i18n;
mod claim;
mod flowlog;
mod oauth;
mod probe;
mod quota;
mod store;
mod zcrypto;

use serde_json::{json, Value};
use std::sync::Mutex;
use store::*;
use tauri::menu::{MenuBuilder, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::AutoLaunchManager;
use tauri_plugin_dialog::DialogExt;

const TRAY_ID: &str = "main";

static STORE_LOCK: Mutex<()> = Mutex::new(());

struct PendingClaim {
    account_id: String,
    account_name: String,
    plan_id: String,
    plan_name: String,
    credentials: Value,
    config: Option<Value>,
    device_mid: String,
}

static PENDING_CLAIM: Mutex<Option<PendingClaim>> = Mutex::new(None);

fn pending_guard() -> std::sync::MutexGuard<'static, Option<PendingClaim>> {
    match PENDING_CLAIM.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn store_guard() -> std::sync::MutexGuard<'static, ()> {
    match STORE_LOCK.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect()
}

fn tray_menu_inner(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let paths = Paths::detect();
    let state = match store::get_state(&paths) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("tray state error: {e}");
            return MenuBuilder::new(app)
                .item(&MenuItem::with_id(app, "show", i18n::tr("tray.show"), true, None::<&str>)?)
                .item(&MenuItem::with_id(app, "quit", i18n::tr("tray.quit"), true, None::<&str>)?)
                .build();
        }
    };

    let b = MenuBuilder::new(app)
        .item(&MenuItem::with_id(app, "show", i18n::tr("tray.show"), true, None::<&str>)?)
        .item(&MenuItem::with_id(app, "capture", i18n::tr("tray.capture"), state.live_logged_in, None::<&str>)?)
        .separator()
        .item(&MenuItem::with_id(app, "launch", i18n::tr("tray.launch"), state.zcode_path_ok && !state.zcode_running, None::<&str>)?)
        .item(&MenuItem::with_id(app, "kill", i18n::tr("tray.kill"), state.zcode_running, None::<&str>)?)
        .separator()
        .item(&MenuItem::with_id(app, "quit", i18n::tr("tray.quit"), true, None::<&str>)?);
    b.build()
}

pub fn rebuild_tray(app: &AppHandle) {
    let app2 = app.clone();
    let res = app.run_on_main_thread(move || {
        if let Some(tray) = app2.tray_by_id(TRAY_ID) {
            let paths = Paths::detect();
            let tip = match store::get_state(&paths) {
                Ok(s) => {
                    let cur = s
                        .accounts
                        .iter()
                        .find(|a| a.is_active)
                        .map(|a| a.name.clone())
                        .or_else(|| s.live_identity.as_ref().and_then(|i| i.label()))
                        .unwrap_or_else(|| if s.live_logged_in { i18n::tr("tray.unsaved") } else { i18n::tr("tray.logged_out") });
                    format!("Z·SWITCH · {cur}")
                }
                Err(_) => "Z·SWITCH".into(),
            };
            let _ = tray.set_tooltip(Some(&tip));
            if let Ok(menu) = tray_menu_inner(&app2) {
                let _ = tray.set_menu(Some(menu));
            }
        }
    });
    if let Err(e) = res {
        eprintln!("rebuild_tray: {e}");
    }
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn run_tray_action(app: AppHandle, action: String) {
    std::thread::spawn(move || {
        let paths = Paths::detect();
        let mut emit_payload = json!({ "action": action });
        let _guard = store_guard();
        let result: Result<serde_json::Value, String> = match action.as_str() {
            "capture" => store::capture_current(&paths, None).map(|a| json!({ "name": a.name })),
            "launch" => {
                let (p, ok) = effective_zcode_path(&paths);
                if ok { store::launch_zcode(&p).map(|_| json!({})) } else { Err(i18n::trf("err.zcode.path_invalid", &[("p", &p)])) }
            }
            "kill" => match store::kill_zcode() {
                Ok(true) => Ok(json!({})),
                Ok(false) => Err(i18n::tr("err.zcode.kill_timeout")),
                Err(e) => Err(e),
            },
            _ => Ok(json!({})),
        };
        match result {
            Ok(v) => {
                emit_payload["ok"] = json!(true);
                emit_payload["result"] = v;
            }
            Err(e) => {
                emit_payload["ok"] = json!(false);
                emit_payload["error"] = json!(e);
            }
        }
        let _ = app.emit("tray-action", &emit_payload);
        rebuild_tray(&app);
    });
}

#[tauri::command]
async fn get_state() -> Result<AppState, String> {
    store::get_state(&Paths::detect())
}

#[tauri::command]
fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

#[tauri::command]
async fn capture_current(app: AppHandle, name: Option<String>) -> Result<Account, String> {
    let _guard = store_guard();
    let r = store::capture_current(&Paths::detect(), name);
    rebuild_tray(&app);
    r
}

#[tauri::command]
async fn rename_account(app: AppHandle, id: String, name: String) -> Result<Account, String> {
    let _guard = store_guard();
    let r = store::rename_account(&Paths::detect(), &id, &name);
    rebuild_tray(&app);
    r
}

#[tauri::command]
async fn delete_account(app: AppHandle, id: String) -> Result<(), String> {
    let _guard = store_guard();
    let r = store::delete_account(&Paths::detect(), &id);
    rebuild_tray(&app);
    r
}

#[tauri::command]
async fn update_account_from_live(app: AppHandle, id: String) -> Result<Account, String> {
    let _guard = store_guard();
    let r = store::update_account_from_live(&Paths::detect(), &id);
    rebuild_tray(&app);
    r
}

#[tauri::command]
async fn switch_to(app: AppHandle, id: String, force: bool, restart: bool) -> Result<SwitchResult, String> {
    let _guard = store_guard();
    let paths = Paths::detect();
    let hot = load_settings(&paths).hot_switch();
    let r = store::switch_to(&paths, &id, force, restart, hot);
    rebuild_tray(&app);
    r
}

#[tauri::command]
async fn get_live_quota() -> Result<quota::QuotaOverview, String> {
    store::live_quota(&Paths::detect())
}

#[tauri::command]
async fn get_account_quota(id: String) -> Result<quota::QuotaOverview, String> {
    store::account_quota(&Paths::detect(), &id)
}

#[tauri::command]
async fn claim_preview(id: String) -> Result<Vec<claim::ClaimPlan>, String> {
    let paths = Paths::detect();
    let mid = store::ensure_virtual_device_mid(&paths, &id)?;
    let acc = load_account(&paths, &id)?;
    claim::preview_plans(&paths.home, &acc.credentials, acc.config.as_ref(), Some(mid))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaimRefreshResult {
    plans: Vec<claim::ClaimPlan>,
    activated: bool,
    activation_error: Option<String>,
}

#[tauri::command]
async fn claim_refresh(id: String) -> Result<ClaimRefreshResult, String> {
    let paths = Paths::detect();
    let mid = store::ensure_virtual_device_mid(&paths, &id)?;
    let acc = load_account(&paths, &id)?;
    let (activated, activation_error) =
        match claim::telemetry_user_id(&paths.home, &acc.credentials) {
            Some(uid) => match claim::report_activation_events(&uid, &mid) {
                Ok(()) => (true, None),
                Err(e) => (false, Some(e)),
            },
            None => (false, None),
        };
    let plans = claim::preview_plans(&paths.home, &acc.credentials, acc.config.as_ref(), Some(mid))?;
    Ok(ClaimRefreshResult { plans, activated, activation_error })
}

#[tauri::command]
async fn claim_start(
    app: AppHandle,
    id: String,
    plan_id: String,
    auto: Option<bool>,
) -> Result<serde_json::Value, String> {
    let paths = Paths::detect();
    let mid = store::ensure_virtual_device_mid(&paths, &id)?;
    let acc = load_account(&paths, &id)?;
    let plans = claim::preview_plans(&paths.home, &acc.credentials, acc.config.as_ref(), Some(mid.clone()))?;
    let plan = plans
        .iter()
        .find(|p| p.plan_id == plan_id)
        .ok_or_else(|| i18n::tr("err.claim.gone"))?;
    let display = if plan.name.is_empty() { plan.plan_id.clone() } else { plan.name.clone() };

    *pending_guard() = Some(PendingClaim {
        account_id: acc.id.clone(),
        account_name: acc.name.clone(),
        plan_id: plan.plan_id.clone(),
        plan_name: display.clone(),
        credentials: acc.credentials,
        config: acc.config,
        device_mid: mid,
    });
    open_captcha_window(&app, auto.unwrap_or(false))?;
    Ok(json!({ "account": acc.name, "plan": display }))
}

#[tauri::command]
async fn claim_captcha_config() -> Result<claim::CaptchaConfig, String> {
    claim::fetch_captcha_config()
}

#[tauri::command]
async fn claim_captcha_submit(
    app: AppHandle,
    param: String,
    region: Option<String>,
) -> Result<serde_json::Value, String> {
    let pending = pending_guard().take().ok_or_else(|| i18n::tr("err.claim.none_pending"))?;
    let paths = Paths::detect();
    let res = claim::submit_claim(
        &paths.home,
        &pending.credentials,
        pending.config.as_ref(),
        &pending.plan_id,
        &param,
        region.as_deref(),
        Some(pending.device_mid),
    );
    close_captcha_window(&app);
    let payload = match res {
        Ok(v) => {
            let ms = |k: &str| -> Option<i64> {
                v.pointer(&format!("/data/plan/{k}"))
                    .and_then(|x| x.as_i64())
                    .map(|s| s * 1000)
            };
            let server_time = v
                .pointer("/data/server_time")
                .and_then(|x| x.as_i64())
                .map(|s| s * 1000);
            let outcome = claim::ClaimOutcome {
                account_id: pending.account_id.clone(),
                account_name: pending.account_name.clone(),
                plan_name: pending.plan_name.clone(),
                starts_at: ms("starts_at"),
                ends_at: ms("ends_at"),
                server_time,
            };
            let p = serde_json::to_value(&outcome).unwrap_or(Value::Null);
            let _ = app.emit("claim://result", &p);
            p
        }
        Err(e) => {
            let p = claim::failure_payload(
                &pending.account_id,
                &pending.account_name,
                &pending.plan_name,
                &e,
            );
            let _ = app.emit("claim://result", &p);
            return Ok(p);
        }
    };
    Ok(payload)
}

#[tauri::command]
async fn claim_cancel(app: AppHandle) -> Result<(), String> {
    *pending_guard() = None;
    close_captcha_window(&app);
    Ok(())
}

fn close_captcha_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("captcha") {
        let _ = w.close();
    }
}

struct PendingOAuth {
    provider: String,
    state: Option<String>,
    flow: String,
    mid: String,
    persisting: bool,
}

#[derive(Clone)]
struct PollCfg {
    url: String,
    token: String,
    expires_at_ms: u128,
    interval_ms: u64,
}

static PENDING_OAUTH: Mutex<Option<PendingOAuth>> = Mutex::new(None);

fn pending_oauth_guard() -> std::sync::MutexGuard<'static, Option<PendingOAuth>> {
    match PENDING_OAUTH.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub(crate) fn begin_pending_flow(flow: &str, provider: &str, mid: &str) {
    let mut pending = pending_oauth_guard();
    *pending = Some(PendingOAuth {
        provider: provider.to_string(),
        state: None,
        flow: flow.to_string(),
        mid: mid.to_string(),
        persisting: false,
    });
    flowlog::log(flow, "begin", &format!("provider={provider}"));
}

pub(crate) fn attach_flow_state(flow: &str, state: &str) -> bool {
    let mut pending = pending_oauth_guard();
    match pending.as_mut() {
        Some(p) if p.flow == flow => {
            p.state = Some(state.to_string());
            true
        }
        _ => false,
    }
}

pub(crate) fn cancel_flow(flow: Option<&str>) -> bool {
    let mut pending = pending_oauth_guard();
    if let Some(p) = pending.as_ref() {
        if flow.map(|f| f == p.flow).unwrap_or(true) {
            let flow_id = p.flow.clone();
            *pending = None;
            flowlog::log(&flow_id, "cancelled", "");
            return true;
        }
    }
    false
}

pub(crate) fn clear_flow_if_matching(flow: &str) -> bool {
    let mut pending = pending_oauth_guard();
    if pending.as_ref().map(|p| p.flow == flow).unwrap_or(false) {
        *pending = None;
        true
    } else {
        false
    }
}

pub(crate) fn is_flow_active(flow: &str) -> bool {
    pending_oauth_guard()
        .as_ref()
        .map(|p| p.flow == flow)
        .unwrap_or(false)
}

pub(crate) fn commit_account_save<F>(flow: &str, save_fn: F) -> Result<serde_json::Value, String>
where
    F: FnOnce() -> Result<serde_json::Value, String>,
{
    let _store_lock = store_guard();
    let mut pending = pending_oauth_guard();
    match pending.as_mut() {
        Some(p) if p.flow == flow => {
            p.persisting = true;
        }
        _ => return Err("__superseded__".into()),
    }
    let res = save_fn();
    match res {
        Ok(v) => {
            if pending.as_ref().map(|p| p.flow == flow).unwrap_or(false) {
                *pending = None;
            }
            Ok(v)
        }
        Err(e) => {
            if let Some(p) = pending.as_mut() {
                if p.flow == flow {
                    p.persisting = false;
                }
            }
            Err(e)
        }
    }
}

#[tauri::command]
async fn oauth_providers() -> Result<Vec<oauth::OAuthProvider>, String> {
    Ok(oauth::OAUTH_PROVIDERS.to_vec())
}

#[tauri::command]
async fn oauth_begin(app: AppHandle, provider: String, flow: Option<String>) -> Result<serde_json::Value, String> {
    if !oauth::OAUTH_PROVIDERS.iter().any(|p| p.id == provider) {
        return Err(i18n::trf("err.oauth.unknown_provider", &[("provider", &provider)]));
    }
    if let Some(w) = app.get_webview_window("login") {
        let _ = w.close();
    }
    let flow = flow
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let mid = uuid::Uuid::new_v4().to_string();

    begin_pending_flow(&flow, &provider, &mid);

    let (p_init, m_init) = (provider.clone(), mid.clone());
    let init = match tauri::async_runtime::spawn_blocking(move || oauth::init_flow(&p_init, &m_init)).await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            clear_flow_if_matching(&flow);
            flowlog::log(&flow, "init-fail", "");
            return Err(e);
        }
        Err(e) => {
            clear_flow_if_matching(&flow);
            let m = i18n::trf("err.oauth.flow", &[("e", &e.to_string())]);
            flowlog::log(&flow, "init-fail", "");
            return Err(m);
        }
    };

    if !attach_flow_state(&flow, &init.state) {
        flowlog::log(&flow, "poll-exit", "superseded");
        return Err("__superseded__".into());
    }

    flowlog::log(&flow, "init-ok", "");

    let poll_cfg = PollCfg {
        url: init.poll_url.clone(),
        token: init.poll_token.clone(),
        expires_at_ms: init.expires_at_ms,
        interval_ms: init.poll_interval_ms,
    };

    spawn_poll_loop(app.clone(), provider.clone(), flow.clone(), mid, poll_cfg);

    Ok(json!({
        "flow": flow,
        "provider": provider,
        "authorize_url": init.authorize_url,
        "expires_at_ms": init.expires_at_ms,
        "poll_interval_ms": init.poll_interval_ms,
    }))
}

#[tauri::command]
async fn oauth_cancel(flow: Option<String>) -> Result<(), String> {
    cancel_flow(flow.as_deref());
    Ok(())
}

#[tauri::command]
async fn oauth_manual_callback(app: AppHandle, flow: String, callback_url: String) -> Result<serde_json::Value, String> {
    let (provider, state, mid) = {
        let pending = pending_oauth_guard();
        match pending.as_ref() {
            Some(p) if p.flow == flow => {
                let Some(state) = &p.state else {
                    return Err(i18n::tr("err.oauth.not_ready"));
                };
                (p.provider.clone(), state.clone(), p.mid.clone())
            }
            _ => return Err(i18n::tr("err.oauth.expired")),
        }
    };
    let res = finish_oauth(&app, provider, state, flow, mid, &callback_url).await;
    match res {
        Ok(v) => Ok(v),
        Err(e) if e == "__superseded__" => Err(i18n::tr("err.oauth.expired")),
        Err(e) if e == "__attribution__" => Err(i18n::tr("err.oauth.bad_cb")),
        Err(e) => Err(e),
    }
}

#[tauri::command]
async fn set_auth_proxy(app: AppHandle, on: bool, url: Option<String>) -> Result<(), String> {
    {
        let _guard = store_guard();
        let paths = Paths::detect();
        let trimmed = url.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let normalized = match trimmed {
            Some(s) => Some(oauth::parse_proxy_url(s)?),
            None => None,
        };
        if on && normalized.is_none() {
            return Err(i18n::tr("err.proxy.need_url"));
        }
        let mut s = load_settings(&paths);
        s.auth_proxy_on = Some(on);
        s.auth_proxy_url = normalized;
        save_settings(&paths, &s)?;
    }
    let _ = app.emit("state-changed", ());
    Ok(())
}

async fn finish_oauth(app: &AppHandle, provider: String, state: String, flow: String, mid: String, callback_url: &str) -> Result<serde_json::Value, String> {
    let result = {
        let provider = provider.clone();
        let state = state.clone();
        let flow = flow.clone();
        let mid = mid.clone();
        let callback_url = callback_url.to_string();
        tauri::async_runtime::spawn_blocking(move || -> Result<serde_json::Value, String> {
            {
                let pending = pending_oauth_guard();
                match pending.as_ref() {
                    Some(p) if p.provider == provider && p.state.as_deref() == Some(&state) && p.flow == flow => {}
                    Some(_) | None => {
                        flowlog::log(&flow, "callback-superseded", "");
                        return Err("__superseded__".into());
                    }
                }
            }
            let (code, cb_state) = match oauth::parse_callback(&callback_url) {
                Ok(oauth::CallbackKind::Code { code, state }) => (code, state),
                Ok(oauth::CallbackKind::Attribution) => {
                    flowlog::log(&flow, "callback-attribution", "");
                    return Err("__attribution__".into());
                }
                Err(e) => {
                    flowlog::log(&flow, "callback-parse-fail", "");
                    return Err(e);
                }
            };
            if cb_state != state {
                flowlog::log(&flow, "callback-state-mismatch", "");
                return Err(i18n::tr("err.oauth.state"));
            }
            let exchanged = match oauth::exchange_token(&provider, &code, &state, &mid) {
                Ok(v) => v,
                Err(e) => {
                    flowlog::log(&flow, "exchange-fail", "");
                    return Err(e);
                }
            };
            match persist_oauth_account(&Paths::detect(), &provider, &exchanged["raw"], &flow, &mid, false) {
                Ok(v) => {
                    flowlog::log(
                        &flow,
                        "persist-ok",
                        &format!("channel=manual duplicate={}", v.get("duplicate").is_some()),
                    );
                    Ok(v)
                }
                Err(e) => {
                    if e != "__superseded__" {
                        flowlog::log(&flow, "persist-fail", "channel=manual");
                    }
                    Err(e)
                }
            }
        })
        .await
        .unwrap_or_else(|e| Err(i18n::trf("err.oauth.flow", &[("e", &e.to_string())])))
    };
    if let Err(e) = &result {
        if deeplink_err_soft(e) {
            let ours = pending_oauth_guard().as_ref().map(|p| p.flow == flow).unwrap_or(false);
            if ours {
                flowlog::log(&flow, "soft-fail", "");
                let _ = app.emit("oauth://done", &json!({ "ok": false, "soft": true, "error": e, "flow": flow }));
            }
            return result;
        }
    }
    finalize_oauth_result(app, &flow, result.clone());
    result
}

fn deeplink_err_soft(e: &str) -> bool {
    e != "__superseded__" && e != "__attribution__"
}

fn persist_oauth_account(
    paths: &Paths,
    provider: &str,
    raw: &serde_json::Value,
    flow: &str,
    mid: &str,
    poll_ready: bool,
) -> Result<serde_json::Value, String> {
    let flow_still_ours = || {
        pending_oauth_guard()
            .as_ref()
            .map(|p| p.flow == flow)
            .unwrap_or(false)
    };
    if !flow_still_ours() {
        return Err("__superseded__".into());
    }
    let jwt = raw
        .pointer("/data/token")
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| i18n::tr("err.oauth.no_token"))?
        .to_string();
    let raw_access = oauth::extract_access_token(provider, raw).unwrap_or_default();
    let access_token = if provider == "zai" && !raw_access.is_empty() {
        oauth::resolve_zai_business_token(&raw_access).ok_or_else(|| i18n::tr("err.oauth.zai_business"))?
    } else {
        raw_access
    };
    let userinfo = if poll_ready {
        oauth::extract_poll_user_profile(raw)
    } else {
        oauth::extract_user_profile(provider, raw)
    }
    .or_else(|| {
        (!access_token.is_empty())
            .then(|| oauth::fetch_userinfo(provider, &access_token))
            .flatten()
    });
    let refresh_token = oauth::extract_refresh_token(provider, raw);
    let credentials = oauth::assemble_credentials_with_token(
        provider,
        &jwt,
        userinfo.as_ref(),
        (!access_token.is_empty()).then_some(access_token.as_str()),
        refresh_token.as_deref(),
    );
    let config = oauth::assemble_config(provider, &jwt, &access_token);

    commit_account_save(flow, || {
        let accounts = list_accounts(paths)?;
        let hash = canonical_hash(&credentials);
        if let Some(i) = store::find_same_login(&credentials, &hash, &accounts, &paths.home) {
            let mut dup = accounts[i].clone();
            dup.hash = hash.clone();
            dup.credentials = credentials;
            dup.config = Some(config);
            dup.updated_at = now_ts();
            if dup.virtual_device_mid.as_deref().is_none_or(|m| m.trim().is_empty()) {
                dup.virtual_device_mid = Some(mid.to_string());
            }
            save_account(paths, &dup)?;
            return Ok(json!({ "id": dup.id, "name": dup.name, "provider": provider, "duplicate": true, "flow": flow }));
        }
        let base = credentials
            .get(format!("oauth:{provider}:user_info"))
            .and_then(|v| v.as_str())
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .and_then(|u| u.get("username").and_then(|x| x.as_str()).map(String::from))
            .unwrap_or_else(|| match provider {
                "zai" => "z.ai".to_string(),
                _ => "BigModel".to_string(),
            });
        let name = unique_name(&accounts, &base);
        let ts = now_ts();
        let acc = Account {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.clone(),
            created_at: ts.clone(),
            updated_at: ts,
            hash,
            credentials,
            config: Some(config),
            virtual_device_mid: Some(mid.to_string()),
            virtual_arms_uid: Some(store::new_arms_uid()),
        };
        save_account(paths, &acc)?;
        Ok(json!({ "id": acc.id, "name": acc.name, "provider": provider, "flow": flow }))
    })
}

fn finalize_oauth_result(app: &AppHandle, flow: &str, result: Result<serde_json::Value, String>) {
    if let Err(e) = &result {
        if e == "__superseded__" || e == "__attribution__" {
            return;
        }
    }
    if let Some(w) = app.get_webview_window("login") {
        let _ = w.close();
    }
    let payload = match result {
        Ok(mut v) => {
            if let Some(obj) = v.as_object_mut() {
                obj.insert("ok".into(), json!(true));
                obj.insert("flow".into(), json!(flow));
            }
            v
        }
        Err(e) => json!({ "ok": false, "error": e, "flow": flow }),
    };
    let _ = app.emit("oauth://done", &payload);
}

fn spawn_poll_loop(app: AppHandle, provider: String, flow: String, mid: String, cfg: PollCfg) {
    std::thread::spawn(move || {
        let deadline = {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            std::cmp::min(cfg.expires_at_ms, now + u128::from(oauth::FLOW_TIMEOUT_MS))
        };
        loop {
            if !is_flow_active(&flow) {
                flowlog::log(&flow, "poll-exit", "flow-done-or-replaced");
                return;
            }
            match oauth::poll_flow_once(&cfg.url, &cfg.token, &mid) {
                Ok(oauth::PollOutcome::Pending) => {}
                Ok(oauth::PollOutcome::Ready(data)) => {
                    flowlog::log(&flow, "poll-ready", "");
                    let raw = json!({ "code": 0, "data": data });
                    let result = persist_oauth_account(&Paths::detect(), &provider, &raw, &flow, &mid, true);
                    match &result {
                        Ok(v) => flowlog::log(
                            &flow,
                            "persist-ok",
                            &format!("channel=poll duplicate={}", v.get("duplicate").is_some()),
                        ),
                        Err(e) if e != "__superseded__" => {
                            flowlog::log(&flow, "persist-fail", "channel=poll");
                        }
                        Err(_) => {}
                    }
                    finalize_oauth_result(&app, &flow, result);
                    return;
                }
                Err(e) => {
                    if !clear_flow_if_matching(&flow) {
                        flowlog::log(&flow, "poll-exit", "superseded");
                        return;
                    }
                    flowlog::log(&flow, "poll-fail", "");
                    finalize_oauth_result(&app, &flow, Err(e));
                    return;
                }
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            if now >= deadline {
                if !clear_flow_if_matching(&flow) {
                    return;
                }
                flowlog::log(&flow, "poll-timeout", "");
                finalize_oauth_result(&app, &flow, Err(i18n::tr("err.oauth.expired")));
                return;
            }
            let sleep = (deadline - now).min(cfg.interval_ms as u128) as u64;
            std::thread::sleep(std::time::Duration::from_millis(sleep));
        }
    });
}

fn open_captcha_window(app: &AppHandle, auto: bool) -> Result<(), String> {
    let (w, h) = (380.0, 320.0);
    if let Some(win) = app.get_webview_window("captcha") {
        let _ = win.eval("location.reload()");
        center_over_main(app, &win, w, h);
        if auto {
            let _ = win.hide();
        } else {
            let _ = win.show();
            let _ = win.set_focus();
        }
        return Ok(());
    }
    let win = tauri::WebviewWindowBuilder::new(
        app,
        "captcha",
        tauri::WebviewUrl::App("captcha.html".into()),
    )
    .title(i18n::tr("title.captcha"))
    .theme(Some(tauri::Theme::Dark))
    .background_color(tauri::window::Color(10, 10, 12, 255))
    .inner_size(w, h)
    .min_inner_size(340.0, 280.0)
    .maximizable(false)
    .resizable(false)
    .additional_browser_args("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --no-proxy-server")
    .visible(false)
    .build()
    .map_err(|e| e.to_string())?;
    center_over_main(app, &win, w, h);
    if !auto {
        let _ = win.show();
        let _ = win.set_focus();
    }
    Ok(())
}

#[tauri::command]
async fn kill_zcode(app: AppHandle) -> Result<(), String> {
    let _guard = store_guard();
    let r = if store::kill_zcode()? { Ok(()) } else { Err(i18n::tr("err.zcode.kill_timeout")) };
    rebuild_tray(&app);
    r
}

#[tauri::command]
async fn set_behavior(
    app: AppHandle,
    launch_after_switch: Option<bool>,
    close_to_tray: Option<bool>,
    hot_switch: Option<bool>,
    auto_claim: Option<bool>,
) -> Result<(), String> {
    let _guard = store_guard();
    let paths = Paths::detect();
    let mut s = load_settings(&paths);
    if let Some(v) = launch_after_switch {
        s.launch_after_switch = Some(v);
    }
    if let Some(v) = close_to_tray {
        s.close_to_tray = Some(v);
    }
    if let Some(v) = hot_switch {
        s.hot_switch = Some(v);
    }
    if let Some(v) = auto_claim {
        s.auto_claim = Some(v);
    }
    let r = save_settings(&paths, &s);
    rebuild_tray(&app);
    let _ = app.emit("state-changed", ());
    r
}

#[tauri::command]
async fn set_language(app: AppHandle, lang: String) -> Result<(), String> {
    let l = i18n::Lang::parse(&lang)
        .ok_or_else(|| i18n::trf("err.lang.unknown", &[("lang", &lang)]))?;
    {
        let _guard = store_guard();
        let paths = Paths::detect();
        let mut s = load_settings(&paths);
        s.language = Some(l.as_str().to_string());
        save_settings(&paths, &s)?;
    }
    i18n::set(l);
    rebuild_tray(&app);
    for (label, key) in [("settings", "title.settings"), ("captcha", "title.captcha"), ("login", "title.login")] {
        if let Some(w) = app.get_webview_window(label) {
            let _ = w.set_title(&i18n::tr(key));
        }
    }
    let _ = app.emit("state-changed", ());
    Ok(())
}

#[tauri::command]
async fn reveal_main(app: AppHandle) -> Result<(), String> {
    let win = app.get_webview_window("main").ok_or_else(|| i18n::tr("err.main.missing"))?;
    win.show().map_err(|e| e.to_string())?;
    let _ = win.set_focus();
    Ok(())
}

#[tauri::command]
async fn open_settings(app: AppHandle) -> Result<(), String> {
    let (w, h) = (520.0, 700.0);
    if let Some(win) = app.get_webview_window("settings") {
        center_over_main(&app, &win, w, h);
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        return Ok(());
    }
    let win = tauri::WebviewWindowBuilder::new(
        &app,
        "settings",
        tauri::WebviewUrl::App("settings.html".into()),
    )
    .title(i18n::tr("title.settings"))
    .theme(Some(tauri::Theme::Dark))
    .background_color(tauri::window::Color(10, 10, 12, 255))
    .inner_size(w, h)
    .min_inner_size(440.0, 540.0)
    .resizable(true)
    .additional_browser_args("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --no-proxy-server")
    .visible(false)
    .build()
    .map_err(|e| e.to_string())?;
    center_over_main(&app, &win, w, h);
    let _ = win.show();
    let _ = win.set_focus();
    Ok(())
}

fn center_over_main(app: &AppHandle, win: &tauri::WebviewWindow, w: f64, h: f64) {
    let Some(m) = app.get_webview_window("main") else { return };
    let (Ok(p), Ok(s), Ok(scale)) = (m.outer_position(), m.outer_size(), m.scale_factor()) else { return };
    let (mx, my) = (p.x as f64 / scale, p.y as f64 / scale);
    let (mw, mh) = (s.width as f64 / scale, s.height as f64 / scale);
    let x = mx + (mw - w).max(0.0) / 2.0;
    let y = my + (mh - h).max(0.0) / 2.0;
    let _ = win.set_position(tauri::LogicalPosition::new(x, y));
}

#[tauri::command]
async fn autostart_status(app: AppHandle) -> Result<bool, String> {
    let al = app.state::<AutoLaunchManager>();
    al.is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
async fn autostart_set(app: AppHandle, enable: bool) -> Result<bool, String> {
    let al = app.state::<AutoLaunchManager>();
    if enable {
        al.enable().map_err(|e| e.to_string())?;
    } else {
        al.disable().map_err(|e| e.to_string())?;
    }
    al.is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
async fn export_pick_path(app: AppHandle, id: String) -> Result<serde_json::Value, String> {
    let acc = load_account(&Paths::detect(), &id)?;
    let default_name = format!("{}.zsb", sanitize_filename(&acc.name));
    let picked = app
        .dialog()
        .file()
        .add_filter(i18n::tr("dialog.zsb"), &["zsb"])
        .set_file_name(&default_name)
        .blocking_save_file();
    let Some(fp) = picked else {
        return Ok(json!({ "picked": false }));
    };
    let path = fp.into_path().map_err(|e| i18n::trf("err.path.invalid", &[("e", &e.to_string())]))?;
    Ok(json!({ "picked": true, "path": path.to_string_lossy(), "name": acc.name }))
}

#[tauri::command]
async fn export_finalize(path: String, id: String, password: String) -> Result<serde_json::Value, String> {
    let acc = load_account(&Paths::detect(), &id)?;
    let payload = store::export_bundle_value(std::slice::from_ref(&acc));
    let sealed = cipher::seal(&payload, &password, cipher::FORMAT_BUNDLE)?;
    let body = serde_json::to_string_pretty(&sealed).unwrap() + "\n";
    store::atomic_write(std::path::Path::new(&path), &body).map_err(|e| i18n::trf("err.write", &[("e", &e.to_string())]))?;
    Ok(json!({ "saved": true, "path": path }))
}

#[tauri::command]
async fn export_all_pick_path(app: AppHandle) -> Result<serde_json::Value, String> {
    let accounts = list_accounts(&Paths::detect())?;
    if accounts.is_empty() {
        return Err(i18n::tr("err.export.empty"));
    }
    let picked = app
        .dialog()
        .file()
        .add_filter(i18n::tr("dialog.zsb"), &["zsb"])
        .set_file_name("zcode-accounts.zsb")
        .blocking_save_file();
    let Some(fp) = picked else {
        return Ok(json!({ "picked": false }));
    };
    let path = fp.into_path().map_err(|e| i18n::trf("err.path.invalid", &[("e", &e.to_string())]))?;
    Ok(json!({ "picked": true, "path": path.to_string_lossy(), "count": accounts.len() }))
}

#[tauri::command]
async fn export_all_finalize(path: String, password: String) -> Result<serde_json::Value, String> {
    let accounts = list_accounts(&Paths::detect())?;
    if accounts.is_empty() {
        return Err(i18n::tr("err.export.empty_short"));
    }
    let payload = store::export_bundle_value(&accounts);
    let sealed = cipher::seal(&payload, &password, cipher::FORMAT_BUNDLE)?;
    let body = serde_json::to_string_pretty(&sealed).unwrap() + "\n";
    store::atomic_write(std::path::Path::new(&path), &body).map_err(|e| i18n::trf("err.write", &[("e", &e.to_string())]))?;
    Ok(json!({ "saved": true, "path": path, "count": accounts.len() }))
}

#[tauri::command]
async fn import_pick_files(app: AppHandle) -> Result<serde_json::Value, String> {
    let picked = app
        .dialog()
        .file()
        .add_filter(i18n::tr("dialog.zsb"), &["zsb"])
        .blocking_pick_files();
    let Some(files) = picked else {
        return Ok(json!({ "picked": false }));
    };
    let mut sealed = vec![];
    let mut errors = vec![];
    for fp in files {
        let path = match fp.into_path() {
            Ok(p) => p,
            Err(e) => {
                errors.push(i18n::trf("err.path.conv", &[("e", &e.to_string())]));
                continue;
            }
        };
        let fname = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let raw = match std::fs::read_to_string(&path) {
            Ok(r) => r,
            Err(e) => {
                errors.push(i18n::trf("err.import.read", &[("fname", fname.as_str()), ("e", &e.to_string())]));
                continue;
            }
        };
        match serde_json::from_str::<Value>(&raw) {
            Ok(v) => {
                if !cipher::is_sealed(&v) {
                    errors.push(i18n::trf("err.import.not_sealed", &[("fname", fname.as_str())]));
                } else if v.get("format").and_then(|f| f.as_str()) != Some(cipher::FORMAT_BUNDLE) {
                    errors.push(i18n::trf("err.import.not_bundle", &[("fname", fname.as_str())]));
                } else {
                    sealed.push((fname, v));
                }
            }
            Err(e) => errors.push(i18n::trf("err.import.json", &[("fname", fname.as_str()), ("e", &e.to_string())])),
        }
    }
    Ok(json!({ "picked": true, "sealed": sealed, "errors": errors }))
}

#[tauri::command]
async fn import_sealed(app: AppHandle, files: Vec<(String, Value)>, password: String) -> Result<ImportReport, String> {
    let _guard = store_guard();
    let mut decrypted = vec![];
    let mut errors = vec![];
    for (fname, v) in files {
        match cipher::open(&v, &password) {
            Ok(payload) => decrypted.push((fname, payload)),
            Err(e) => errors.push(i18n::trf("err.import.wrap", &[("fname", fname.as_str()), ("e", &e)])),
        }
    }
    let mut report = if decrypted.is_empty() {
        ImportReport::default()
    } else {
        import_values(&Paths::detect(), &decrypted)?
    };
    report.picked = true;
    report.errors.extend(errors);
    rebuild_tray(&app);
    Ok(report)
}

#[tauri::command]
async fn pick_zcode_path(app: AppHandle) -> Result<serde_json::Value, String> {
    let picked = app
        .dialog()
        .file()
        .add_filter(i18n::tr("dialog.exe"), &["exe"])
        .blocking_pick_file();
    let Some(fp) = picked else {
        return Ok(json!({ "picked": false }));
    };
    let path = fp.into_path().map_err(|e| i18n::trf("err.path.invalid", &[("e", &e.to_string())]))?;
    Ok(json!({ "picked": true, "path": path.to_string_lossy() }))
}

#[tauri::command]
async fn set_zcode_path(app: AppHandle, path: String) -> Result<(), String> {
    let _guard = store_guard();
    let paths = Paths::detect();
    let mut s = load_settings(&paths);
    s.zcode_path = store::normalize_zcode_path(&path);
    let r = save_settings(&paths, &s);
    rebuild_tray(&app);
    let _ = app.emit("state-changed", ());
    r
}

#[tauri::command]
async fn launch_zcode(app: AppHandle) -> Result<(), String> {
    let paths = Paths::detect();
    let (p, ok) = effective_zcode_path(&paths);
    if !ok {
        return Err(i18n::trf("err.zcode.path_invalid_hint", &[("p", &p)]));
    }
    let r = store::launch_zcode(&p);
    rebuild_tray(&app);
    r
}

#[tauri::command]
async fn open_external(url: String) -> Result<(), String> {
    store::open_url(&url)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            get_state,
            app_version,
            capture_current,
            rename_account,
            delete_account,
            update_account_from_live,
            switch_to,
            get_live_quota,
            get_account_quota,
            claim_preview,
            claim_refresh,
            claim_start,
            claim_captcha_config,
            claim_captcha_submit,
            claim_cancel,
            oauth_providers,
            oauth_begin,
            oauth_cancel,
            oauth_manual_callback,
            set_auth_proxy,
            kill_zcode,
            set_behavior,
            set_language,
            autostart_status,
            autostart_set,
            export_pick_path,
            export_finalize,
            export_all_pick_path,
            export_all_finalize,
            import_pick_files,
            import_sealed,
            pick_zcode_path,
            set_zcode_path,
            launch_zcode,
            open_external,
            open_settings,
            reveal_main,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    let paths = Paths::detect();
                    if store::load_settings(&paths).close_to_tray() {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
                if window.label() == "captcha" {
                    *pending_guard() = None;
                }
            }
        })
        .setup(|app| {
            i18n::init_from_settings(&store::load_settings(&Paths::detect()));
            if let Ok(data_dir) = app.path().app_local_data_dir() {
                flowlog::init(&data_dir);
            }
            let _tray = TrayIconBuilder::with_id(TRAY_ID)
                .icon(app.default_window_icon().expect("no window icon").clone())
                .tooltip("Z·SWITCH")
                .menu(&tray_menu_inner(app.handle())?)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| {
                    let id = event.id().as_ref().to_string();
                    match id.as_str() {
                        "show" => show_main(app),
                        "quit" => app.exit(0),
                        other => run_tray_action(app.clone(), other.to_string()),
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click { button: tauri::tray::MouseButton::Left, button_state: tauri::tray::MouseButtonState::Up, .. } = event {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod oauth_flow_tests {
    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_begin_and_cancel_lifecycle() {
        let _lock = TEST_LOCK.lock().unwrap();
        let flow1 = "flow-test-lifecycle-1";
        let flow2 = "flow-test-lifecycle-2";

        // 1. Begin flow 1 via production helper
        begin_pending_flow(flow1, "bigmodel", "mid-1");
        assert!(is_flow_active(flow1));

        // 2. Cancellation with wrong flow ID does not clear flow 1
        assert!(!cancel_flow(Some("wrong-flow")));
        assert!(is_flow_active(flow1));

        // 3. Flow 2 supersedes flow 1 via production helper
        begin_pending_flow(flow2, "zai", "mid-2");
        assert!(!is_flow_active(flow1));
        assert!(is_flow_active(flow2));

        // 4. Old flow 1 cancellation does not clear flow 2
        assert!(!cancel_flow(Some(flow1)));
        assert!(is_flow_active(flow2));

        // 5. Correct cancellation clears flow 2
        assert!(cancel_flow(Some(flow2)));
        assert!(!is_flow_active(flow2));
        assert!(pending_oauth_guard().is_none());
    }

    #[test]
    fn test_superseded_flow_cannot_attach_state() {
        let _lock = TEST_LOCK.lock().unwrap();
        let flow1 = "flow-init-ord-1";
        let flow2 = "flow-init-ord-2";

        // Flow 1 begins
        begin_pending_flow(flow1, "bigmodel", "mid-1");

        // Before flow 1 attaches state, user starts flow 2
        begin_pending_flow(flow2, "zai", "mid-2");

        // Flow 1 late init resolution must be rejected
        assert!(!attach_flow_state(flow1, "state-1"));

        // Flow 2 init resolution must be accepted
        assert!(attach_flow_state(flow2, "state-2"));

        // Clean up
        cancel_flow(None);
    }

    #[test]
    fn test_termination_paths_never_clear_new_flow() {
        let _lock = TEST_LOCK.lock().unwrap();
        let flow_old = "flow-term-old";
        let flow_new = "flow-term-new";

        // Old flow begins
        begin_pending_flow(flow_old, "bigmodel", "mid-old");

        // New flow begins, superseding old flow
        begin_pending_flow(flow_new, "zai", "mid-new");

        // Old poll failure/timeout path attempts to clear using production helper:
        assert!(!clear_flow_if_matching(flow_old));

        // New flow must remain active and unharmed
        assert!(is_flow_active(flow_new));

        // Matching termination clears cleanly
        assert!(clear_flow_if_matching(flow_new));
        assert!(!is_flow_active(flow_new));
    }

    #[test]
    fn test_commit_save_mutual_exclusion_with_cancel() {
        let _lock = TEST_LOCK.lock().unwrap();
        let flow = "flow-commit-cancel";

        begin_pending_flow(flow, "bigmodel", "mid-test");
        assert!(attach_flow_state(flow, "state-test"));

        // User cancels flow before persistence commit
        assert!(cancel_flow(Some(flow)));

        // commit_account_save must return Err("__superseded__") and NEVER execute closure
        let mut executed = false;
        let res = commit_account_save(flow, || {
            executed = true;
            Ok(json!({"saved": true}))
        });
        assert_eq!(res.unwrap_err(), "__superseded__");
        assert!(!executed);
    }

    #[test]
    fn test_commit_save_concurrent_poll_and_callback() {
        let _lock = TEST_LOCK.lock().unwrap();
        let flow = "flow-comp-test";

        begin_pending_flow(flow, "bigmodel", "mid-comp");
        assert!(attach_flow_state(flow, "state-comp"));

        let flow_t1 = flow.to_string();
        let flow_t2 = flow.to_string();

        let handle1 = std::thread::spawn(move || {
            commit_account_save(&flow_t1, || Ok(json!({"winner": 1})))
        });
        let handle2 = std::thread::spawn(move || {
            commit_account_save(&flow_t2, || Ok(json!({"winner": 2})))
        });

        let res1 = handle1.join().unwrap();
        let res2 = handle2.join().unwrap();

        // Exactly one must succeed, and the other must return Err("__superseded__")
        let (ok_count, superseded_count) = match (&res1, &res2) {
            (Ok(_), Err(e)) if e == "__superseded__" => (1, 1),
            (Err(e), Ok(_)) if e == "__superseded__" => (1, 1),
            _ => (0, 0),
        };
        assert_eq!(ok_count, 1, "exactly one commit must succeed");
        assert_eq!(superseded_count, 1, "the loser must receive __superseded__");

        // The flow must now be cleared
        assert!(!is_flow_active(flow));
    }

    #[test]
    fn test_commit_save_failure_resets_persisting_flag() {
        let _lock = TEST_LOCK.lock().unwrap();
        let flow = "flow-retry-test";

        begin_pending_flow(flow, "bigmodel", "mid-fail");
        assert!(attach_flow_state(flow, "state-fail"));

        // Disk save fails
        let fail_res = commit_account_save(flow, || Err("simulated disk error".into()));
        assert!(fail_res.is_err());

        // Flow is still active and persisting flag was reset so retry works
        assert!(is_flow_active(flow));

        let retry_res = commit_account_save(flow, || Ok(json!({"saved": true})));
        assert!(retry_res.is_ok());
        assert!(!is_flow_active(flow));
    }
}
