
use crate::i18n::{tr, trf};
use crate::quota;
use crate::zcrypto;
use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use uuid::Uuid;

#[cfg(windows)]
fn no_window(prog: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new(prog);
    c.creation_flags(0x0800_0000);
    c
}
#[cfg(not(windows))]
fn no_window(prog: &str) -> std::process::Command {
    std::process::Command::new(prog)
}

#[cfg(windows)]
fn detached(mut c: std::process::Command) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    c.creation_flags(0x0000_0008 | 0x0000_0200);
    c
}
#[cfg(not(windows))]
fn detached(mut c: std::process::Command) -> std::process::Command {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    c.process_group(0);
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    c
}

pub struct Paths {
    pub home: PathBuf,
    pub data_root: PathBuf,
}

pub(crate) fn pick_home(zswitch: Option<PathBuf>, userprofile: Option<PathBuf>, home_env: Option<PathBuf>) -> PathBuf {
    zswitch
        .or(userprofile)
        .or(home_env)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn abs_env_path(name: &str) -> Option<PathBuf> {
    let raw = std::env::var(name).ok()?;
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let p = PathBuf::from(t);
    p.is_absolute().then_some(p)
}

fn bootstrap_data_base_dir(home: &Path) -> Option<PathBuf> {
    fs::read_to_string(home.join(".zcode").join("v2").join("setting.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.get("dataBaseDir").and_then(|d| d.as_str()).map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .filter(|d| d.is_absolute())
}

pub(crate) fn resolve_data_root(home: &Path) -> PathBuf {
    if let Some(d) = abs_env_path("ZCODE_SWITCH_DATA_ROOT") {
        return d;
    }
    if let Some(d) = bootstrap_data_base_dir(home) {
        return d;
    }
    if let Some(d) = abs_env_path("ZCODE_DATA_BASE_DIR") {
        return d;
    }
    home.to_path_buf()
}

impl Paths {
    pub fn detect() -> Paths {
        let home = pick_home(
            std::env::var("ZCODE_SWITCH_HOME").ok().map(PathBuf::from),
            std::env::var("USERPROFILE").ok().map(PathBuf::from),
            std::env::var("HOME").ok().map(PathBuf::from),
        );
        let data_root = resolve_data_root(&home);
        Paths { home, data_root }
    }

    pub fn store_dir(&self) -> PathBuf { self.home.join(".zcode-switch") }
    pub fn accounts_dir(&self) -> PathBuf { self.store_dir().join("accounts") }
    pub fn settings_file(&self) -> PathBuf { self.store_dir().join("settings.json") }
    pub fn zcode_dir(&self) -> PathBuf { self.data_root.join(".zcode") }
    pub fn live_file(&self) -> PathBuf { self.zcode_dir().join("v2").join("credentials.json") }
    pub fn live_config(&self) -> PathBuf { self.zcode_dir().join("v2").join("config.json") }
    pub fn live_telemetry(&self) -> PathBuf { self.zcode_dir().join("v2").join("telemetry-state.json") }
    pub fn live_setting(&self) -> PathBuf { self.home.join(".zcode").join("v2").join("setting.json") }
    pub fn live_plan_cache(&self) -> PathBuf { self.zcode_dir().join("v2").join("coding-plan-cache.json") }

    pub fn ensure_dirs(&self) -> Result<(), String> {
        fs::create_dir_all(self.accounts_dir()).map_err(|e| trf("err.store.mk_accounts_dir", &[("e", &e.to_string())]))?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
    pub hash: String,
    pub credentials: Value,
    #[serde(default)]
    pub config: Option<Value>,
    #[serde(default)]
    pub virtual_device_mid: Option<String>,
    #[serde(default)]
    pub virtual_arms_uid: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Settings {
    pub zcode_path: Option<String>,
    pub launch_after_switch: Option<bool>,
    pub close_to_tray: Option<bool>,
    #[serde(default)]
    pub hot_switch: Option<bool>,
    #[serde(default)]
    pub auth_proxy_on: Option<bool>,
    #[serde(default)]
    pub auth_proxy_url: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub auto_claim: Option<bool>,
}

impl Settings {
    pub fn launch_after_switch(&self) -> bool { self.launch_after_switch.unwrap_or(true) }
    pub fn close_to_tray(&self) -> bool { self.close_to_tray.unwrap_or(true) }
    pub fn hot_switch(&self) -> bool { self.hot_switch.unwrap_or(false) }
    pub fn auto_claim(&self) -> bool { self.auto_claim.unwrap_or(false) }
    pub fn auth_proxy(&self) -> Option<&str> {
        if self.auth_proxy_on.unwrap_or(false) {
            self.auth_proxy_url.as_deref().map(str::trim).filter(|s| !s.is_empty())
        } else {
            None
        }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct AccountSummary {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
    pub is_active: bool,
    pub has_config: bool,
    pub has_user_info: bool,
    pub jwt_expired: bool,
    pub identity: zcrypto::Identity,
}

#[derive(Serialize, Clone, Debug)]
pub struct AppState {
    pub zcode_running: bool,
    pub live_exists: bool,
    pub live_logged_in: bool,
    pub live_hash: Option<String>,
    pub active_account_id: Option<String>,
    pub live_identity: Option<zcrypto::Identity>,
    pub accounts: Vec<AccountSummary>,
    pub zcode_path: String,
    pub zcode_path_ok: bool,
    pub store_dir: String,
    pub launch_after_switch: bool,
    pub close_to_tray: bool,
    pub hot_switch: bool,
    pub auto_claim: bool,
    pub auth_proxy_on: bool,
    pub auth_proxy_url: Option<String>,
    pub language: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct SwitchResult {
    pub switched: bool,
    pub already_active: bool,
    pub name: String,
    pub preserved_as: Option<String>,
    pub killed: bool,
    pub launched: bool,
    pub hot: bool,
    #[serde(default)]
    pub config_stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<crate::probe::ProbeOutcome>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct ImportReport {
    pub picked: bool,
    pub added: Vec<String>,
    pub skipped: Vec<String>,
    pub errors: Vec<String>,
}

pub fn now_ts() -> String {
    Local::now().format("%Y-%m-%d %H:%M").to_string()
}

const DEVICE_KEY_PREFIX: &str = "web-remote-control:";

pub fn canonical_hash(v: &Value) -> String {
    let filtered = match v.as_object() {
        Some(map) if map.keys().any(|k| k.starts_with(DEVICE_KEY_PREFIX)) => {
            let kept: serde_json::Map<String, Value> = map
                .iter()
                .filter(|(k, _)| !k.starts_with(DEVICE_KEY_PREFIX))
                .map(|(k, val)| (k.clone(), val.clone()))
                .collect();
            Value::Object(kept)
        }
        _ => v.clone(),
    };
    let bytes = serde_json::to_vec(&filtered).unwrap_or_default();
    let d = Sha256::digest(&bytes);
    format!("{d:x}")
}

pub fn is_logged_in(v: &Value) -> bool {
    let Some(map) = v.as_object() else { return false };
    if map.keys().any(|k| k.starts_with("oauth:") && k.ends_with(":access_token")) {
        return true;
    }
    map.get("zcodejwttoken")
        .and_then(|t| t.as_str())
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false)
}

pub fn atomic_write(path: &Path, data: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| trf("err.mkdir", &[("e", &e.to_string())]))?;
    }
    let tmp = path.with_extension(format!("tmp-{}", Uuid::new_v4().simple()));
    fs::write(&tmp, data).map_err(|e| trf("err.write_file", &[("path", &path.display().to_string()), ("e", &e.to_string())]))?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(trf("err.rename_fail", &[("path", &path.display().to_string()), ("e", &e.to_string())]));
    }
    Ok(())
}

pub fn read_live(paths: &Paths) -> Result<Option<Value>, String> {
    if !paths.live_file().exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(paths.live_file())
        .map_err(|e| trf("err.read", &[("path", &paths.live_file().display().to_string()), ("e", &e.to_string())]))?;
    let v: Value = serde_json::from_str(&raw)
        .map_err(|e| trf("err.bad_json", &[("path", &paths.live_file().display().to_string()), ("e", &e.to_string())]))?;
    if !v.is_object() {
        return Err(tr("err.store.not_object"));
    }
    Ok(Some(v))
}

pub fn read_live_config(paths: &Paths) -> Option<Value> {
    let raw = fs::read_to_string(paths.live_config()).ok()?;
    serde_json::from_str(&raw).ok()
}

pub(crate) fn new_gen_provider_config(data_root: &Path) -> bool {
    data_root.join(".zcode").join("v2").join("provider_config.json").exists()
}

fn snapshot_config_from_live(paths: &Paths) -> Option<Value> {
    if new_gen_provider_config(&paths.data_root) { None } else { read_live_config(paths) }
}

pub fn write_live(paths: &Paths, v: &Value) -> Result<(), String> {
    if let Some(parent) = paths.live_file().parent() {
        fs::create_dir_all(parent).map_err(|e| trf("err.mkdir", &[("e", &e.to_string())]))?;
    }
    let body = serde_json::to_string_pretty(v).unwrap_or_default() + "\n";
    atomic_write(&paths.live_file(), &body)
}

pub fn write_live_config(paths: &Paths, v: &Value) -> Result<(), String> {
    let body = serde_json::to_string_pretty(v).unwrap_or_default() + "\n";
    atomic_write(&paths.live_config(), &body)
}

pub(crate) fn in_sandbox() -> bool {
    std::env::var("ZCODE_SWITCH_HOME").is_ok()
}

#[cfg(windows)]
pub fn zcode_running() -> bool {
    if in_sandbox() {
        return false;
    }
    let out = no_window("tasklist")
        .args(["/FI", "IMAGENAME eq ZCode.exe", "/FO", "CSV", "/NH"])
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .any(|l| l.to_lowercase().starts_with("\"zcode.exe\"")),
        Err(_) => false,
    }
}

#[cfg(not(windows))]
pub fn zcode_running() -> bool {
    if in_sandbox() {
        return false;
    }
    ["zcode", "ZCode"].iter().any(|name| {
        no_window("pgrep")
            .args(["-x", name])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

#[cfg(windows)]
pub fn kill_zcode() -> Result<bool, String> {
    if in_sandbox() {
        return Ok(true);
    }
    if !zcode_running() {
        return Ok(true);
    }
    let _ = no_window("taskkill")
        .args(["/F", "/IM", "ZCode.exe"])
        .output();
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if !zcode_running() {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    Ok(!zcode_running())
}

#[cfg(not(windows))]
pub fn kill_zcode() -> Result<bool, String> {
    if in_sandbox() {
        return Ok(true);
    }
    if !zcode_running() {
        return Ok(true);
    }
    for name in ["zcode", "ZCode"] {
        let _ = no_window("pkill").args(["-x", name]).output();
    }
    let soft_deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < soft_deadline {
        if !zcode_running() {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    if zcode_running() {
        for name in ["zcode", "ZCode"] {
            let _ = no_window("pkill").args(["-9", "-x", name]).output();
        }
    }
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        if !zcode_running() {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    Ok(!zcode_running())
}

pub fn launch_zcode(path: &str) -> Result<(), String> {
    if in_sandbox() {
        return Ok(());
    }
    let p = PathBuf::from(path);
    if !p.exists() {
        return Err(trf("err.zcode.missing", &[("path", path)]));
    }
    detached(Command::new(&p))
        .spawn()
        .map_err(|e| trf("err.zcode.launch", &[("e", &e.to_string())]))?;
    Ok(())
}

pub fn open_url(url: &str) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("仅支持 https 链接".into());
    }
    if in_sandbox() {
        return Ok(());
    }
    #[cfg(windows)]
    let cmd = {
        let mut c = no_window("cmd");
        c.args(["/c", "start", "", url]);
        c
    };
    #[cfg(target_os = "macos")]
    let cmd = {
        let mut c = no_window("open");
        c.arg(url);
        c
    };
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let cmd = {
        let mut c = no_window("xdg-open");
        c.arg(url);
        c
    };
    let _ = detached(cmd).spawn();
    Ok(())
}

pub fn load_settings(paths: &Paths) -> Settings {
    match fs::read_to_string(paths.settings_file()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            eprintln!("settings.json 损坏，已回退默认值：{e}");
            Settings::default()
        }),
        Err(_) => Settings::default(),
    }
}

pub fn save_settings(paths: &Paths, s: &Settings) -> Result<(), String> {
    paths.ensure_dirs()?;
    let body = serde_json::to_string_pretty(s).unwrap_or_default() + "\n";
    atomic_write(&paths.settings_file(), &body)
}

pub fn client_path_candidates(os: &str) -> Vec<String> {
    match os {
        "macos" => vec![
            "/Applications/ZCode.app/Contents/MacOS/ZCode".to_string(),
            format!("{}/Applications/ZCode.app/Contents/MacOS/ZCode", std::env::var("HOME").unwrap_or_default()),
            "/usr/local/bin/zcode".to_string(),
        ],
        "windows" => vec![
            r"C:\Program Files\ZCode\ZCode.exe".to_string(),
            std::env::var("LOCALAPPDATA")
                .map(|l| format!(r"{}\Programs\ZCode\ZCode.exe", l))
                .unwrap_or_default(),
        ],
        _ => vec![
            "/usr/local/bin/zcode".to_string(),
            "/usr/bin/zcode".to_string(),
            "/opt/ZCode/zcode".to_string(),
            format!("{}/.local/bin/zcode", std::env::var("HOME").unwrap_or_default()),
        ],
    }
}

pub fn normalize_zcode_path(raw: &str) -> Option<String> {
    let mut t = raw.trim();
    if t.len() >= 2 {
        let b = t.as_bytes();
        let quoted = (b[0] == b'"' && b[t.len() - 1] == b'"') || (b[0] == b'\'' && b[t.len() - 1] == b'\'');
        if quoted {
            t = t[1..t.len() - 1].trim();
        }
    }
    (!t.is_empty()).then(|| t.to_string())
}

pub fn effective_zcode_path(paths: &Paths) -> (String, bool) {
    effective_zcode_path_in(paths, &client_path_candidates(std::env::consts::OS))
}

fn effective_zcode_path_in(paths: &Paths, candidates: &[String]) -> (String, bool) {
    let s = load_settings(paths);
    if let Some(p) = s.zcode_path.as_ref().filter(|p| PathBuf::from(p).exists()) {
        return (p.clone(), true);
    }
    for c in candidates {
        if !c.is_empty() && PathBuf::from(c).exists() {
            return (c.clone(), true);
        }
    }
    if let Some(p) = s.zcode_path {
        return (p, false);
    }
    (candidates.first().cloned().unwrap_or_default(), false)
}

pub fn list_accounts(paths: &Paths) -> Result<Vec<Account>, String> {
    let dir = paths.accounts_dir();
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut out = vec![];
    for entry in fs::read_dir(&dir).map_err(|e| trf("err.store.list_fail", &[("e", &e.to_string())]))? {
        let entry = entry.map_err(|e| trf("err.store.list_fail", &[("e", &e.to_string())]))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(mut a) = serde_json::from_str::<Account>(&raw) {
                migrate_account_hash_inplace(&mut a);
                out.push(a);
            }
        }
    }
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.name.cmp(&b.name)));
    Ok(out)
}

pub fn save_account(paths: &Paths, acc: &Account) -> Result<(), String> {
    paths.ensure_dirs()?;
    let path = paths.accounts_dir().join(format!("{}.json", acc.id));
    let body = serde_json::to_string_pretty(acc).unwrap_or_default() + "\n";
    atomic_write(&path, &body)
}

pub fn load_account(paths: &Paths, id: &str) -> Result<Account, String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(tr("err.store.bad_id"));
    }
    let path = paths.accounts_dir().join(format!("{id}.json"));
    let raw = fs::read_to_string(&path).map_err(|_| trf("err.store.no_account_id", &[("id", id)]))?;
    let mut a: Account = serde_json::from_str(&raw).map_err(|e| trf("err.store.corrupt", &[("e", &e.to_string())]))?;
    migrate_account_hash_inplace(&mut a);
    Ok(a)
}

fn migrate_account_hash_inplace(acc: &mut Account) {
    let want = canonical_hash(&acc.credentials);
    if want != acc.hash {
        acc.hash = want;
    }
}

fn name_exists(accounts: &[Account], name: &str) -> bool {
    accounts.iter().any(|a| a.name.eq_ignore_ascii_case(name))
}

pub fn unique_name(accounts: &[Account], base: &str) -> String {
    if !name_exists(accounts, base) {
        return base.to_string();
    }
    for n in 2..1000 {
        let cand = format!("{base} {n}");
        if !name_exists(accounts, &cand) {
            return cand;
        }
    }
    format!("{base} {}", Uuid::new_v4().simple())
}

pub fn capture_current(paths: &Paths, name: Option<String>) -> Result<Account, String> {
    let live = read_live(paths)?.ok_or(tr("err.live.no_creds_file"))?;
    if !is_logged_in(&live) {
        return Err(tr("err.live.no_credentials"));
    }
    let hash = canonical_hash(&live);
    let accounts = list_accounts(paths)?;
    if let Some(i) = find_same_login(&live, &hash, &accounts, &paths.home) {
        return Err(trf("err.live.dup_saved", &[("name", &accounts[i].name)]));
    }
    let config = snapshot_config_from_live(paths);
    let name = match name {
        Some(n) => unique_name(&accounts, &n),
        None => {
            let id = zcrypto::account_identity(&live, &paths.home);
            unique_name(&accounts, &id.label().unwrap_or_else(|| "Account 1".into()))
        }
    };
    let ts = now_ts();
    let mut acc = Account {
        id: Uuid::new_v4().to_string(),
        name,
        created_at: ts.clone(),
        updated_at: ts,
        hash,
        credentials: live,
        config,
        virtual_device_mid: None,
        virtual_arms_uid: None,
    };
    adopt_virtual_device_mid(paths, &mut acc)?;
    adopt_virtual_arms_uid(paths, &mut acc)?;
    Ok(acc)
}

pub(crate) fn find_same_login(live: &Value, live_hash: &str, accounts: &[Account], home: &Path) -> Option<usize> {
    if let Some(i) = accounts.iter().position(|a| a.hash == live_hash) {
        return Some(i);
    }
    let live_id = zcrypto::account_identity(live, home);
    if !identity_has_signal(&live_id) {
        return None;
    }
    accounts.iter().position(|a| {
        identity_matches(&live_id, &zcrypto::account_identity(&a.credentials, home))
    })
}

fn auto_preserve(paths: &Paths, accounts: &[Account], target_hash: &str) -> Result<Option<String>, String> {
    let live = match read_live(paths)? {
        Some(v) if is_logged_in(&v) => v,
        _ => return Ok(None),
    };
    let hash = canonical_hash(&live);
    if hash == target_hash {
        return Ok(None);
    }
    if find_same_login(&live, &hash, accounts, &paths.home).is_some() {
        return Ok(None);
    }
    let name = unique_name(accounts, &format!("Auto {}", Local::now().format("%m-%d %H%M")));
    let ts = now_ts();
    let mut acc = Account {
        id: Uuid::new_v4().to_string(),
        name: name.clone(),
        created_at: ts.clone(),
        updated_at: ts,
        hash,
        credentials: live,
        config: snapshot_config_from_live(paths),
        virtual_device_mid: None,
        virtual_arms_uid: None,
    };
    adopt_virtual_device_mid(paths, &mut acc)?;
    adopt_virtual_arms_uid(paths, &mut acc)?;
    Ok(Some(name))
}

fn cred_plain(creds: &Value, key: &str, home: &std::path::Path) -> Option<String> {
    let v = creds.get(key)?.as_str()?;
    if zcrypto::is_encrypted(v) {
        zcrypto::decrypt_with_secret(v, &zcrypto::default_secret(home)).ok()
    } else {
        Some(v.to_string())
    }
}

pub(crate) const JWT_CLOCK_SKEW_MS: i64 = 30_000;

pub(crate) fn account_jwt_expired(creds: &Value, home: &Path) -> bool {
    let Some(jwt) = cred_plain(creds, "zcodejwttoken", home) else { return false };
    let Some(exp) = zcrypto::jwt_exp_ms(&jwt) else { return false };
    let now_ms = Local::now().timestamp_millis();
    exp <= now_ms + JWT_CLOCK_SKEW_MS
}

fn sync_live_back_to_source(paths: &Paths, accounts: &[Account]) -> Result<(), String> {
    let Some(live) = read_live(paths)? else { return Ok(()) };
    if !is_logged_in(&live) { return Ok(()); }
    let live_hash = canonical_hash(&live);
    let live_id = zcrypto::account_identity(&live, &paths.home);
    let has_id = identity_has_signal(&live_id);
    let source = accounts.iter().find(|a| a.hash == live_hash).cloned().or_else(|| {
        if !has_id { return None; }
        accounts.iter().find(|a| {
            identity_matches(&live_id, &zcrypto::account_identity(&a.credentials, &paths.home))
        }).cloned()
    });
    let Some(mut src) = source else { return Ok(()); };
    let mut changed = false;
    if src.credentials != live {
        src.credentials = live.clone();
        src.hash = live_hash;
        changed = true;
    }
    if !new_gen_provider_config(&paths.data_root) && src.config.is_some() {
        let cfg = read_live_config(paths);
        if cfg.is_some() && cfg != src.config {
            src.config = cfg;
            changed = true;
        }
    }
    if changed {
        src.updated_at = now_ts();
        save_account(paths, &src)?;
    }
    Ok(())
}

fn reset_live_plan_cache(paths: &Paths) {
    let p = paths.live_plan_cache();
    if p.exists() {
        if let Err(e) = fs::remove_file(&p) {
            eprintln!("删除 coding-plan-cache.json 失败(忽略): {e}");
        }
    }
}

fn align_family_domain(paths: &Paths, target: &Account) {
    let Some(provider) = cred_plain(&target.credentials, "oauth:active_provider", &paths.home)
        .filter(|p| p == "bigmodel" || p == "zai") else { return; };
    let Ok(raw) = fs::read_to_string(paths.live_setting()) else { return; };
    let Ok(mut v) = serde_json::from_str::<Value>(&raw) else {
        eprintln!("setting.json 解析失败,跳过 family domain 对齐");
        return;
    };
    let Some(obj) = v.as_object_mut() else { return; };
    let now_ms = chrono::Local::now().timestamp_millis();
    obj.insert("providerFamilyDomain".into(), Value::String(provider));
    obj.insert("providerFamilyDomainUpdatedAt".into(), Value::from(now_ms));
    let body = serde_json::to_string_pretty(&v).unwrap_or_default() + "\n";
    if let Err(e) = atomic_write(&paths.live_setting(), &body) {
        eprintln!("setting.json family domain 写回失败(忽略): {e}");
    }
}

fn rematerialize_wiped_builtins(paths: &Paths, target: &Account) {
    if target.config.is_none() { return; }
    let Some(provider) = cred_plain(&target.credentials, "oauth:active_provider", &paths.home)
        .filter(|p| p == "bigmodel" || p == "zai") else { return; };
    let Some(jwt) = cred_plain(&target.credentials, "zcodejwttoken", &paths.home)
        .filter(|j| !j.trim().is_empty()) else { return; };
    let Some(live) = read_live_config(paths) else { return; };
    let mut out = match live.as_object() { Some(o) => o.clone(), None => return };
    let Some(live_prov) = out.get("provider").and_then(|v| v.as_object()) else { return };

    let wiped = |cur: &Value| {
        cur.get("options").and_then(|o| o.get("apiKey"))
            .map(|k| k.as_str().map(str::trim).unwrap_or("").is_empty())
            .unwrap_or(true)
            || (cur.get("enabled").and_then(|e| e.as_bool()) == Some(false)
                && matches!(
                    cur.get("systemDisabledReason").and_then(|s| s.as_str()),
                    Some("oauth_provider_inactive") | Some("coding_plan_auth_failed")
                ))
    };
    let family_prefix = format!("builtin:{provider}");

    let has_candidate = live_prov
        .iter()
        .any(|(id, cur)| id.starts_with(&family_prefix) && wiped(cur));
    if !has_candidate { return; }

    let at_key = format!("oauth:{provider}:access_token");
    let access_token = match (in_sandbox(), cred_plain(&target.credentials, &at_key, &paths.home)) {
        (true, _) => String::new(),
        (false, Some(at)) => at.trim().to_string(),
        (false, None) => String::new(),
    };
    let access_token = if provider == "zai" && !access_token.is_empty() {
        crate::oauth::resolve_zai_business_token(&access_token).unwrap_or(access_token)
    } else {
        access_token
    };
    let fresh = crate::oauth::assemble_config(&provider, &jwt, &access_token);
    let Some(fresh_map) = fresh.get("provider").and_then(|v| v.as_object()) else { return; };
    let mut live_prov = live_prov.clone();
    let mut changed = false;
    for (id, fresh_entry) in fresh_map {
        let Some(cur) = live_prov.get(id) else { continue; };
        if !wiped(cur) { continue; }
        let Some(new_key) = fresh_entry.pointer("/options/apiKey")
            .and_then(|k| k.as_str()).map(str::trim)
            .filter(|k| !k.is_empty()) else { continue; };
        let mut patched = cur.clone();
        if let Some(opts) = patched.get_mut("options").and_then(|o| o.as_object_mut()) {
            opts.insert("apiKey".into(), Value::String(new_key.to_string()));
            opts.remove("apiKeyRequired");
        }
        if let Some(obj) = patched.as_object_mut() {
            obj.insert("enabled".into(), Value::Bool(true));
            obj.remove("systemDisabledReason");
        }
        live_prov.insert(id.clone(), patched);
        changed = true;
    }
    if changed {
        out.insert("provider".into(), Value::Object(live_prov));
        let body = serde_json::to_string_pretty(&Value::Object(out)).unwrap_or_default() + "\n";
        if let Err(e) = atomic_write(&paths.live_config(), &body) {
            eprintln!("重物化 builtin apiKey 写回失败(忽略): {e}");
        }
    }
}

fn run_switch_probe(paths: &Paths, target: &Account, mid: Option<String>) -> Option<crate::probe::ProbeOutcome> {
    if in_sandbox() || !is_logged_in(&target.credentials) {
        return None;
    }
    let cfg = probe_config_base(paths, target);
    crate::probe::switch_probe(
        &target.credentials,
        cfg.as_ref(),
        &zcrypto::default_secret(&paths.home),
        mid,
        new_gen_provider_config(&paths.data_root),
    )
}

fn probe_config_base(paths: &Paths, target: &Account) -> Option<Value> {
    if target.config.is_none() { return None; }
    if new_gen_provider_config(&paths.data_root) { target.config.clone() }
    else { read_live_config(paths).or_else(|| target.config.clone()) }
}

pub fn switch_to(paths: &Paths, id: &str, force: bool, restart: bool, hot: bool) -> Result<SwitchResult, String> {
    let target = load_account(paths, id)?;
    let accounts = list_accounts(paths)?;
    let live = read_live(paths)?;
    let live_hash = live.as_ref().map(canonical_hash);

    let already = live_hash.as_deref() == Some(target.hash.as_str())
        || live.as_ref().is_some_and(|v| {
            is_logged_in(v) && {
                let (li, ti) = (
                    zcrypto::account_identity(v, &paths.home),
                    zcrypto::account_identity(&target.credentials, &paths.home),
                );
                identity_has_signal(&li) && identity_has_signal(&ti) && identity_matches(&li, &ti)
            }
        });
    if already {
        if live_hash.as_deref() != Some(target.hash.as_str()) {
            if let Err(e) = sync_live_back_to_source(paths, &accounts) {
                eprintln!("sync-back 失败(不阻断 already 返回): {e}");
            }
        }
        let mid = ensure_virtual_device_mid_locked(paths, &target.id)?;
        write_live_device_mid(paths, &mid)?;
        let uid = ensure_virtual_arms_uid_locked(paths, &target.id)?;
        if !zcode_running() {
            let _ = write_live_arms_uid(paths, &uid);
        }
        return Ok(SwitchResult {
            switched: false,
            already_active: true,
            name: target.name,
            preserved_as: None,
            killed: false,
            launched: false,
            hot: false,
            config_stale: false,
            probe: None,
        });
    }

    let running = zcode_running();
    if hot && running {
        if let Err(e) = sync_live_back_to_source(paths, &accounts) {
            eprintln!("sync-back 失败(不阻断切换): {e}");
        }
        let accounts = list_accounts(paths)?;
        let preserved_as = auto_preserve(paths, &accounts, &target.hash)?;
        let creds = inject_relay_pass_hash(&target.credentials, current_relay_pass(paths).as_ref());
        hot_swap_verified(paths, &target, &creds)?;
        backfill_relay_pass_hash(paths, &target.id, &creds);
        reset_live_plan_cache(paths);
        let mid = ensure_virtual_device_mid_locked(paths, &target.id)?;
        write_live_device_mid(paths, &mid)?;
        ensure_virtual_arms_uid_locked(paths, &target.id)?;
        let probe = run_switch_probe(paths, &target, Some(mid));
        return Ok(SwitchResult {
            switched: true,
            already_active: false,
            name: target.name,
            preserved_as,
            killed: false,
            launched: false,
            hot: true,
            config_stale: target.config.is_none() && paths.live_config().exists(),
            probe,
        });
    }

    let mut killed = false;
    if running {
        if !force {
            return Err(tr("err.switch.running"));
        }
        if !kill_zcode()? {
            return Err(tr("err.switch.kill_timeout"));
        }
        killed = true;
    }

    if let Err(e) = sync_live_back_to_source(paths, &accounts) {
        eprintln!("sync-back 失败(不阻断切换): {e}");
    }
    let accounts = list_accounts(paths)?;

    let preserved_as = auto_preserve(paths, &accounts, &target.hash)?;

    let creds = inject_relay_pass_hash(&target.credentials, current_relay_pass(paths).as_ref());
    write_live(paths, &creds)?;
    backfill_relay_pass_hash(paths, &target.id, &creds);
    let new_gen = new_gen_provider_config(&paths.data_root);
    if new_gen {
        eprintln!(
            "[zsw] 新代际配置（provider_config.json 在）：跳过 config.json 写入与重物化（注册表版本 {}）",
            crate::quota::zcode_app_version()
        );
    }
    if let Some(cfg) = &target.config {
        if !new_gen {
            write_live_config(paths, cfg)?;
        }
    }
    reset_live_plan_cache(paths);
    align_family_domain(paths, &target);
    if !new_gen {
        rematerialize_wiped_builtins(paths, &target);
    }
    let mid = ensure_virtual_device_mid_locked(paths, &target.id)?;
    write_live_device_mid(paths, &mid)?;
    let uid = ensure_virtual_arms_uid_locked(paths, &target.id)?;
    let _ = write_live_arms_uid(paths, &uid);

    let mut launched = false;
    if restart {
        let (p, ok) = effective_zcode_path(paths);
        if ok && launch_zcode(&p).is_ok() {
            launched = true;
        }
    }

    let probe = run_switch_probe(paths, &target, Some(mid));

    Ok(SwitchResult {
        switched: true,
        already_active: false,
        name: target.name,
        preserved_as,
        killed,
        launched,
        hot: false,
        config_stale: target.config.is_none() && paths.live_config().exists(),
        probe,
    })
}

const RELAY_PASS_KEY: &str = "web-remote-control:external-relay:pass_hash";

fn inject_relay_pass_hash(creds: &Value, relay: Option<&Value>) -> Value {
    let Some(relay) = relay else { return creds.clone() };
    let mut out = creds.clone();
    if let Some(map) = out.as_object_mut() {
        map.insert(RELAY_PASS_KEY.to_string(), relay.clone());
    }
    out
}

fn current_relay_pass(paths: &Paths) -> Option<Value> {
    read_live(paths)
        .ok()
        .flatten()
        .and_then(|v| v.get(RELAY_PASS_KEY).cloned())
}

fn backfill_relay_pass_hash(paths: &Paths, target_id: &str, creds: &Value) {
    let Ok(mut acc) = load_account(paths, target_id) else { return };
    if acc.credentials != *creds {
        acc.credentials = creds.clone();
        acc.updated_at = now_ts();
        if let Err(e) = save_account(paths, &acc) {
            eprintln!("relay 回填失败(不阻断切换): {e}");
        }
    }
}

fn hot_swap_verified(paths: &Paths, target: &Account, creds: &Value) -> Result<(), String> {
    let want = zcrypto::account_identity(&target.credentials, &paths.home);
    let use_hash = !identity_has_signal(&want);
    let verify = |v: &Value| {
        if use_hash {
            canonical_hash(v) == target.hash
        } else {
            identity_matches(&want, &zcrypto::account_identity(v, &paths.home))
        }
    };
    let mut last_err: Option<String> = None;
    let backoff = |attempt: u32| std::thread::sleep(std::time::Duration::from_millis(250 + u64::from(attempt) * 250));
    for attempt in 0..3u32 {
        if let Err(e) = write_live(paths, creds) {
            last_err = Some(trf("err.write", &[("e", &e)]));
            backoff(attempt);
            continue;
        }
        if let Some(cfg) = &target.config {
            if !new_gen_provider_config(&paths.data_root) {
                if let Err(e) = write_live_config(paths, cfg) {
                    last_err = Some(trf("err.write_config", &[("e", &e)]));
                    backoff(attempt);
                    continue;
                }
            }
        }
        if let Ok(Some(v)) = read_live(paths) {
            if verify(&v) {
                std::thread::sleep(std::time::Duration::from_millis(150));
                if let Ok(Some(v2)) = read_live(paths) {
                    if verify(&v2) {
                        return Ok(());
                    }
                }
            }
        }
        backoff(attempt);
    }
    Err(last_err.unwrap_or_else(|| tr("err.hot.verify")))
}

fn identity_has_signal(id: &zcrypto::Identity) -> bool {
    id.user_id.as_deref().is_some_and(|s| !s.is_empty())
        || id.email.as_deref().is_some_and(|s| !s.is_empty())
        || id.username.as_deref().is_some_and(|s| !s.is_empty())
}

fn identity_matches(a: &zcrypto::Identity, b: &zcrypto::Identity) -> bool {
    let norm = |s: &str| s.trim().to_lowercase();
    let opt = |s: &Option<String>| norm(s.as_deref().unwrap_or("")).to_string();
    let (au, ae, ap, an) = (opt(&a.user_id), opt(&a.email), norm(&a.provider), opt(&a.username));
    let (bu, be, bp, bn) = (opt(&b.user_id), opt(&b.email), norm(&b.provider), opt(&b.username));
    if !au.is_empty() && !bu.is_empty() {
        if au != bu {
            return false;
        }
        if !ae.is_empty() && !be.is_empty() && ae != be {
            return false;
        }
        return true;
    }
    if !ae.is_empty() && !be.is_empty() {
        return ae == be;
    }
    ap == bp && !an.is_empty() && an == bn
}

pub fn rename_account(paths: &Paths, id: &str, new_name: &str) -> Result<Account, String> {
    let name = new_name.trim();
    if name.is_empty() {
        return Err(tr("err.name.empty"));
    }
    if name.chars().count() > 40 {
        return Err(tr("err.name.too_long"));
    }
    let mut acc = load_account(paths, id)?;
    let accounts = list_accounts(paths)?;
    if let Some(other) = accounts
        .iter()
        .find(|a| a.id != id && a.name.eq_ignore_ascii_case(name))
    {
        return Err(trf("err.name.taken", &[("name", name), ("other", &other.name)]));
    }
    acc.name = name.to_string();
    acc.updated_at = now_ts();
    save_account(paths, &acc)?;
    Ok(acc)
}

pub fn delete_account(paths: &Paths, id: &str) -> Result<(), String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(tr("err.store.bad_id"));
    }
    let path = paths.accounts_dir().join(format!("{id}.json"));
    if !path.exists() {
        return Err(tr("err.store.no_account"));
    }
    fs::remove_file(&path).map_err(|e| trf("err.store.delete_fail", &[("e", &e.to_string())]))
}

pub fn update_account_from_live(paths: &Paths, id: &str) -> Result<Account, String> {
    let live = read_live(paths)?.ok_or(tr("err.live.no_file"))?;
    if !is_logged_in(&live) {
        return Err(tr("err.live.logged_out"));
    }
    let hash = canonical_hash(&live);
    let accounts = list_accounts(paths)?;
    if let Some(i) = find_same_login(&live, &hash, &accounts, &paths.home) {
        if accounts[i].id != id {
            return Err(trf("err.live.same", &[("name", &accounts[i].name)]));
        }
    }
    let mut acc = load_account(paths, id)?;
    acc.hash = hash;
    acc.credentials = live;
    if !new_gen_provider_config(&paths.data_root) {
        acc.config = read_live_config(paths);
    }
    acc.updated_at = now_ts();
    save_account(paths, &acc)?;
    Ok(acc)
}

fn live_quota_config(paths: &Paths) -> Option<Value> {
    if new_gen_provider_config(&paths.data_root) { None } else { read_live_config(paths) }
}

pub fn live_quota(paths: &Paths) -> Result<quota::QuotaOverview, String> {
    let creds = read_live(paths)?.ok_or(tr("err.live.no_file"))?;
    if !is_logged_in(&creds) {
        return Err(tr("err.live.quota"));
    }
    quota::quota_for_live(&paths.home, &creds, live_quota_config(paths).as_ref())
}

pub fn account_quota(paths: &Paths, id: &str) -> Result<quota::QuotaOverview, String> {
    let acc = load_account(paths, id)?;
    quota::quota_for_snapshot(&paths.home, &acc.credentials, acc.config.as_ref())
}

fn ensure_virtual_device_mid_locked(paths: &Paths, id: &str) -> Result<String, String> {
    {
        let acc = load_account(paths, id)?;
        if let Some(m) = acc.virtual_device_mid.clone() {
            if !m.trim().is_empty() {
                return Ok(m);
            }
        }
    }
    let mut acc = load_account(paths, id)?;
    if let Some(m) = acc.virtual_device_mid.clone() {
        if !m.trim().is_empty() {
            return Ok(m);
        }
    }
    let m = Uuid::new_v4().to_string();
    acc.virtual_device_mid = Some(m.clone());
    acc.updated_at = now_ts();
    save_account(paths, &acc)?;
    Ok(m)
}

pub fn ensure_virtual_device_mid(paths: &Paths, id: &str) -> Result<String, String> {
    if let Ok(acc) = load_account(paths, id) {
        if let Some(m) = acc.virtual_device_mid.clone() {
            if !m.trim().is_empty() {
                return Ok(m);
            }
        }
    }
    let _guard = crate::store_guard();
    ensure_virtual_device_mid_locked(paths, id)
}

pub fn write_live_device_mid(paths: &Paths, mid: &str) -> Result<(), String> {
    let mut v: Value = fs::read_to_string(paths.live_telemetry())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));
    if !v.is_object() {
        v = json!({});
    }
    if v.get("deviceMid").and_then(|m| m.as_str()) == Some(mid) {
        return Ok(());
    }
    v["deviceMid"] = json!(mid);
    atomic_write(&paths.live_telemetry(), &(serde_json::to_string(&v).unwrap_or_default() + "\n"))
}

fn adopt_virtual_device_mid(paths: &Paths, acc: &mut Account) -> Result<(), String> {
    if acc.virtual_device_mid.as_deref().map_or(false, |m| !m.trim().is_empty()) {
        return Ok(());
    }
    let live_mid: Option<String> = fs::read_to_string(paths.live_telemetry())
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.get("deviceMid").and_then(|m| m.as_str()).map(String::from))
        .filter(|m| !m.trim().is_empty());
    let taken = |m: &str| {
        list_accounts(paths)
            .map(|accs| accs.iter().any(|a| a.virtual_device_mid.as_deref() == Some(m)))
            .unwrap_or(false)
    };
    let mid = match live_mid {
        Some(m) if !taken(&m) => m,
        _ => Uuid::new_v4().to_string(),
    };
    acc.virtual_device_mid = Some(mid);
    acc.updated_at = now_ts();
    save_account(paths, acc)
}

const ARMS_DEFAULT_STORE_FILE: &str = "ZGVmYXVsdA.json";

pub fn new_arms_uid() -> String {
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let uuid = Uuid::new_v4();
    let suffix: String = uuid.as_bytes()
        .iter()
        .take(16)
        .map(|b| ALPHABET[(*b as usize) % 36] as char)
        .collect();
    format!("uid_{suffix}")
}

fn arms_store_dirs_from(
    win_appdata: Option<PathBuf>,
    mac_appsupport: Option<PathBuf>,
    unix_base: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push_if_new = |dir: PathBuf| {
        let lowered = dir.to_string_lossy().to_lowercase();
        if !out.iter().any(|d| d.to_string_lossy().to_lowercase() == lowered) {
            out.push(dir);
        }
    };
    let mut candidates: Vec<PathBuf> = Vec::new();
    let groups: [(Option<PathBuf>, &[&str]); 3] = [
        (win_appdata, &["ZCode", "zcode", "ZCode Preview", "ZCode Dev"]),
        (mac_appsupport, &["ZCode", "zcode", "ZCode Preview", "ZCode Dev"]),
        (unix_base, &["zcode", "ZCode"]),
    ];
    for (base, names) in groups {
        let Some(base) = base else { continue };
        for name in names {
            candidates.push(base.join(name).join("rum-electron-store"));
        }
        if let Ok(rd) = fs::read_dir(&base) {
            let extras: Vec<PathBuf> = rd
                .flatten()
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().to_lowercase();
                    n.starts_with("zcode") && e.path().join("rum-electron-store").is_dir()
                })
                .map(|e| e.path().join("rum-electron-store"))
                .collect();
            candidates.extend(extras);
        }
    }
    let existing: Vec<PathBuf> = candidates.iter().filter(|c| c.is_dir()).cloned().collect();
    if existing.is_empty() {
        candidates.truncate(1);
        candidates
    } else {
        for c in existing {
            push_if_new(c);
        }
        out
    }
}

fn arms_store_dirs_for(paths: &Paths) -> Vec<PathBuf> {
    if in_sandbox() {
        return vec![paths.home.join("arms-store-sandbox")];
    }
    #[cfg(windows)]
    let (appdata, mac_base, unix_base) =
        (std::env::var("APPDATA").ok().map(PathBuf::from), None, None);
    #[cfg(target_os = "macos")]
    let (appdata, mac_base, unix_base) = (
        None,
        std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join("Library").join("Application Support")),
        None,
    );
    #[cfg(all(unix, not(target_os = "macos")))]
    let (appdata, mac_base, unix_base) = (
        None,
        None,
        std::env::var("XDG_CONFIG_HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config"))),
    );
    arms_store_dirs_from(appdata, mac_base, unix_base)
}

fn ensure_virtual_arms_uid_locked(paths: &Paths, id: &str) -> Result<String, String> {
    {
        let acc = load_account(paths, id)?;
        if let Some(u) = acc.virtual_arms_uid.clone() {
            if !u.trim().is_empty() {
                return Ok(u);
            }
        }
    }
    let mut acc = load_account(paths, id)?;
    if let Some(u) = acc.virtual_arms_uid.clone() {
        if !u.trim().is_empty() {
            return Ok(u);
        }
    }
    let u = new_arms_uid();
    acc.virtual_arms_uid = Some(u.clone());
    acc.updated_at = now_ts();
    save_account(paths, &acc)?;
    Ok(u)
}

fn read_live_arms_uid_from(dirs: &[PathBuf]) -> Option<String> {
    let read_uid = |f: &Path| -> Option<String> {
        fs::read_to_string(f)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v.get("_arms_uid").and_then(|u| u.as_str()).map(String::from))
            .filter(|u| !u.trim().is_empty())
    };
    let mut files: Vec<PathBuf> = Vec::new();
    for d in dirs {
        if let Ok(rd) = fs::read_dir(d) {
            let mut jsons: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
                .collect();
            jsons.sort_by_key(|p| !p.file_name().map(|n| n == ARMS_DEFAULT_STORE_FILE).unwrap_or(false));
            files.extend(jsons);
        }
    }
    files.iter().find_map(|f| read_uid(f))
}

pub fn write_live_arms_uid_to(dirs: &[PathBuf], uid: &str) -> Result<(), String> {
    for d in dirs {
        let mut jsons: Vec<PathBuf> = fs::read_dir(d)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
                    .collect()
            })
            .unwrap_or_default();
        if jsons.is_empty() {
            fs::create_dir_all(d).map_err(|e| format!("mkdir {}: {e}", d.display()))?;
            jsons.push(d.join(ARMS_DEFAULT_STORE_FILE));
        }
        let mut first_err: Option<String> = None;
        for f in jsons {
            let mut v: Value = fs::read_to_string(&f)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| json!({}));
            if !v.is_object() {
                v = json!({});
            }
            if v.get("_arms_uid").and_then(|u| u.as_str()) == Some(uid)
                && v.get("_arms_session").is_none()
            {
                continue;
            }
            let obj = v.as_object_mut().unwrap();
            obj.insert("_arms_uid".into(), json!(uid));
            obj.remove("_arms_session");
            if let Err(e) = atomic_write(&f, &(serde_json::to_string(&v).unwrap_or_default() + "\n")) {
                first_err.get_or_insert(e);
            }
        }
        if let Some(e) = first_err {
            return Err(e);
        }
    }
    Ok(())
}

fn write_live_arms_uid(paths: &Paths, uid: &str) -> Result<(), String> {
    write_live_arms_uid_to(&arms_store_dirs_for(paths), uid)
}

fn adopt_virtual_arms_uid(paths: &Paths, acc: &mut Account) -> Result<(), String> {
    if acc.virtual_arms_uid.as_deref().is_some_and(|u| !u.trim().is_empty()) {
        return Ok(());
    }
    let live_uid = read_live_arms_uid_from(&arms_store_dirs_for(paths));
    let taken = |u: &str| {
        list_accounts(paths)
            .map(|accs| accs.iter().any(|a| a.virtual_arms_uid.as_deref() == Some(u)))
            .unwrap_or(false)
    };
    let uid = match live_uid {
        Some(u) if !taken(&u) => u,
        _ => new_arms_uid(),
    };
    acc.virtual_arms_uid = Some(uid);
    acc.updated_at = now_ts();
    save_account(paths, acc)
}

pub fn export_bundle_value(accounts: &[Account]) -> Value {
    json!({
        "format": "zcode-accounts-bundle",
        "version": 2,
        "exportedAt": now_ts(),
        "accounts": accounts.iter().map(|a| json!({
            "name": a.name,
            "createdAt": a.created_at,
            "credentials": a.credentials,
            "config": a.config,
        })).collect::<Vec<_>>(),
    })
}

type ImportCandidate = (Option<String>, Value, Option<Value>);

fn import_candidates(v: &Value) -> Result<Vec<ImportCandidate>, String> {
    if v.get("format").and_then(|f| f.as_str()) == Some("zcode-accounts-bundle") {
        let arr = v
            .get("accounts")
            .and_then(|a| a.as_array())
            .ok_or(tr("err.bundle.no_accounts"))?;
        let mut out = vec![];
        for item in arr {
            let creds = item.get("credentials").cloned().ok_or(tr("err.bundle.no_creds"))?;
            out.push((
                item.get("name").and_then(|n| n.as_str()).map(String::from),
                creds,
                item.get("config").cloned(),
            ));
        }
        return Ok(out);
    }
    Err(tr("err.bundle.unrecognized"))
}

pub fn import_values(paths: &Paths, files: &[(String, Value)]) -> Result<ImportReport, String> {
    let mut report = ImportReport { picked: true, ..Default::default() };
    let accounts = list_accounts(paths)?;

    let mut new_accounts: Vec<Account> = vec![];

    for (fname, v) in files {
        let cands = match import_candidates(v) {
            Ok(c) => c,
            Err(e) => {
                report.errors.push(trf("err.import.wrap", &[("fname", fname.as_str()), ("e", e.as_str())]));
                continue;
            }
        };
        for (name_opt, creds, config_opt) in cands {
            if !is_logged_in(&creds) {
                report.skipped.push(trf("err.import.no_creds", &[("fname", fname.as_str())]));
                continue;
            }
            let hash = canonical_hash(&creds);
            if accounts.iter().any(|a| a.hash == hash) || new_accounts.iter().any(|a| a.hash == hash) {
                report.skipped.push(trf("err.import.dup", &[("fname", fname.as_str())]));
                continue;
            }
            let base_name = name_opt.unwrap_or_else(|| {
                let id = zcrypto::account_identity(&creds, &paths.home);
                id.label().unwrap_or_else(|| format!("Import {}", Local::now().format("%m-%d %H%M")))
            });
            let name = unique_name(&accounts, &base_name);
            let ts = now_ts();
            let acc = Account {
                id: Uuid::new_v4().to_string(),
                name: name.clone(),
                created_at: ts.clone(),
                updated_at: ts,
                hash,
                credentials: creds,
                config: config_opt,
                virtual_device_mid: None,
                virtual_arms_uid: None,
            };
            new_accounts.push(acc);
            report.added.push(name);
        }
    }

    for acc in &new_accounts {
        save_account(paths, acc)?;
    }
    Ok(report)
}

pub fn get_state(paths: &Paths) -> Result<AppState, String> {
    let accounts = list_accounts(paths)?;
    let live = read_live(paths)?;
    let live_hash = live.as_ref().map(canonical_hash);
    let live_logged_in = live.as_ref().map(is_logged_in).unwrap_or(false);
    let active_account_id = live_hash
        .as_ref()
        .and_then(|h| accounts.iter().find(|a| &a.hash == h).map(|a| a.id.clone()));
    let live_identity = live
        .as_ref()
        .filter(|_| live_logged_in)
        .map(|v| zcrypto::account_identity(v, &paths.home));
    let (zcode_path, zcode_path_ok) = effective_zcode_path(paths);
    let settings = load_settings(paths);
    let summaries = accounts
        .iter()
        .map(|a| AccountSummary {
            id: a.id.clone(),
            name: a.name.clone(),
            created_at: a.created_at.clone(),
            updated_at: a.updated_at.clone(),
            is_active: live_hash.as_deref() == Some(a.hash.as_str()),
            has_config: a.config.is_some(),
            has_user_info: crate::claim::telemetry_user_id(&paths.home, &a.credentials).is_some(),
            jwt_expired: account_jwt_expired(&a.credentials, &paths.home),
            identity: zcrypto::account_identity(&a.credentials, &paths.home),
        })
        .collect();
    Ok(AppState {
        zcode_running: zcode_running(),
        live_exists: paths.live_file().exists(),
        live_logged_in,
        live_hash,
        active_account_id,
        live_identity,
        accounts: summaries,
        zcode_path,
        zcode_path_ok,
        store_dir: paths.store_dir().to_string_lossy().to_string(),
        launch_after_switch: settings.launch_after_switch(),
        close_to_tray: settings.close_to_tray(),
        hot_switch: settings.hot_switch(),
        auto_claim: settings.auto_claim(),
        auth_proxy_on: settings.auth_proxy_on.unwrap_or(false),
        auth_proxy_url: settings.auth_proxy_url.clone(),
        language: crate::i18n::current().as_str().to_string(),
    })
}
