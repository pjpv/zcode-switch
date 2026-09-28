//! 应用内更新：只查询本仓库的 GitHub Releases。
//! 下载地址由 tag 和文件名拼出，不用接口返回的任意 URL；
//! 安装前用同一次发布里的 `checksums-*.txt` 核对 SHA-256。

use serde::Deserialize;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

use crate::i18n::{tr, trf};

const REPO: &str = "pjpv/zcode-switch";
const RELEASES_URL: &str =
    "https://api.github.com/repos/pjpv/zcode-switch/releases?per_page=30";
const UA: &str = concat!("zcode-switch/", env!("CARGO_PKG_VERSION"));
const MAX_INSTALLER: u64 = 80 * 1024 * 1024;
const MAX_CHECKSUM: u64 = 1024 * 1024;
const MAX_API: u64 = 2 * 1024 * 1024;

static INSTALLING: AtomicBool = AtomicBool::new(false);

struct InstallGuard;
impl Drop for InstallGuard {
    fn drop(&mut self) {
        INSTALLING.store(false, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateCheck {
    pub current: String,
    pub latest: String,
    pub available: bool,
    pub prerelease: bool,
    pub notes: String,
    pub html_url: String,
    pub asset_name: String,
    pub mode: String,
    pub installable: bool,
    pub quitting: bool,
    /// "" | "dev" | "no_asset"
    pub reason: String,
    #[serde(skip)]
    tag: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateApply {
    pub mode: String,
    pub quitting: bool,
}

#[derive(Debug, Clone)]
struct ReleaseInfo {
    tag: String,
    version_text: String,
    version: SemVer,
    prerelease: bool,
    draft: bool,
    notes: String,
    assets: Vec<String>,
}

#[derive(Debug, Clone)]
struct Host {
    os: &'static str,
    arch: &'static str,
    portable: bool,
    appimage: bool,
    dev: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SemVer {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<PreId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PreId {
    Num(u64),
    Str(String),
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| cmp_pre(&self.pre, &other.pre))
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn cmp_pre(a: &[PreId], b: &[PreId]) -> std::cmp::Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => std::cmp::Ordering::Equal,
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        (false, false) => {
            for (x, y) in a.iter().zip(b.iter()) {
                let o = cmp_id(x, y);
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            a.len().cmp(&b.len())
        }
    }
}

fn cmp_id(a: &PreId, b: &PreId) -> std::cmp::Ordering {
    match (a, b) {
        (PreId::Num(x), PreId::Num(y)) => x.cmp(y),
        (PreId::Str(x), PreId::Str(y)) => x.cmp(y),
        (PreId::Num(_), PreId::Str(_)) => std::cmp::Ordering::Less,
        (PreId::Str(_), PreId::Num(_)) => std::cmp::Ordering::Greater,
    }
}

fn parse_num(s: &str) -> Option<u64> {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) {
        return None;
    }
    if !s.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn parse_semver(raw: &str) -> Option<SemVer> {
    let s = raw.trim();
    let s = s.strip_prefix('v').or_else(|| s.strip_prefix('V')).unwrap_or(s);
    let s = s.split('+').next().unwrap_or(s);
    if s.is_empty() {
        return None;
    }
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (s, None),
    };
    let mut parts = core.split('.');
    let major = parse_num(parts.next()?)?;
    let minor = parse_num(parts.next()?)?;
    let patch = parse_num(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    let pre = match pre {
        None => Vec::new(),
        Some(p) if p.is_empty() => return None,
        Some(p) => p
            .split('.')
            .map(|id| {
                if id.is_empty() {
                    return None;
                }
                if id.chars().all(|c| c.is_ascii_digit()) {
                    Some(PreId::Num(parse_num(id)?))
                } else if id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                    Some(PreId::Str(id.to_string()))
                } else {
                    None
                }
            })
            .collect::<Option<Vec<_>>>()?,
    };
    Some(SemVer { major, minor, patch, pre })
}

fn version_text(raw: &str) -> String {
    let s = raw.trim();
    s.strip_prefix('v').or_else(|| s.strip_prefix('V')).unwrap_or(s).to_string()
}

fn safe_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 180
        && !s.starts_with('.')
        && !s.ends_with('.')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn clip(s: &str) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= 180 {
            out.push('…');
            break;
        }
        if ch == '\n' || ch == '\r' {
            out.push(' ');
            continue;
        }
        out.push(ch);
    }
    out
}

fn excerpt(body: &str) -> String {
    let t = body.trim().replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::new();
    for (i, ch) in t.chars().enumerate() {
        if i >= 800 {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn releases_page(tag: &str) -> String {
    format!("https://github.com/{REPO}/releases/tag/{tag}")
}

fn latest_page() -> String {
    format!("https://github.com/{REPO}/releases/latest")
}

fn download_url(tag: &str, name: &str) -> Result<String, String> {
    let rest = tag.strip_prefix('v').or_else(|| tag.strip_prefix('V')).unwrap_or("");
    if rest.is_empty() || !safe_token(rest) || !safe_token(name) {
        return Err(tr("err.update.path"));
    }
    Ok(format!("https://github.com/{REPO}/releases/download/{tag}/{name}"))
}

#[derive(Deserialize)]
struct RawAsset {
    name: String,
}

#[derive(Deserialize)]
struct RawRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

fn parse_releases(text: &str) -> Result<Vec<ReleaseInfo>, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|_| tr("err.update.parse"))?;
    if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
        let low = msg.to_ascii_lowercase();
        if low.contains("rate limit") || low.contains("abuse") {
            return Err(tr("err.update.rate"));
        }
        return Err(tr("err.update.parse"));
    }
    let raw: Vec<RawRelease> = serde_json::from_value(v).map_err(|_| tr("err.update.parse"))?;
    let mut out = Vec::new();
    for r in raw {
        let Some(ver) = parse_semver(&r.tag_name) else { continue };
        let Some(rest) = r.tag_name.strip_prefix('v').or_else(|| r.tag_name.strip_prefix('V')) else { continue };
        if !safe_token(rest) {
            continue;
        }
        let assets = r.assets.into_iter().map(|a| a.name).filter(|n| safe_token(n)).collect();
        out.push(ReleaseInfo {
            version_text: version_text(&r.tag_name),
            tag: r.tag_name,
            version: ver,
            prerelease: r.prerelease,
            draft: r.draft,
            notes: excerpt(r.body.as_deref().unwrap_or("")),
            assets,
        });
    }
    Ok(out)
}

fn detect_host(exe: Option<&Path>) -> Host {
    let os = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        "other"
    };
    let portable = exe.map(is_portable_name).unwrap_or(false);
    let appimage = std::env::var_os("APPIMAGE").is_some();
    let dev = cfg!(debug_assertions) || exe.map(path_has_target).unwrap_or(false);
    Host { os, arch, portable, appimage, dev }
}

fn is_portable_name(p: &Path) -> bool {
    p.file_name()
        .and_then(|s| s.to_str())
        .map(|n| n.to_ascii_lowercase().contains("portable"))
        .unwrap_or(false)
}

fn path_has_target(p: &Path) -> bool {
    p.components().any(|c| c.as_os_str() == "target")
}

fn asset_matches(name: &str, mode: &str, arch: &str) -> bool {
    let n = name.to_ascii_lowercase();
    match mode {
        "setup" => arch == "x86_64" && n.ends_with("-setup.exe"),
        "portable" => arch == "x86_64" && n.ends_with("_portable.exe"),
        "dmg" => match arch {
            "aarch64" => n.ends_with("_aarch64.dmg"),
            "x86_64" => n.ends_with("_x64.dmg"),
            _ => false,
        },
        "appimage" => arch == "x86_64" && n.ends_with(".appimage"),
        "deb" => arch == "x86_64" && n.ends_with(".deb"),
        _ => false,
    }
}

fn find_asset<'a>(assets: &'a [String], mode: &str, arch: &str) -> Option<&'a str> {
    assets.iter().find(|n| asset_matches(n, mode, arch)).map(|s| s.as_str())
}

fn preferred_mode(host: &Host) -> &'static str {
    match host.os {
        "windows" if host.portable => "portable",
        "windows" => "setup",
        "macos" => "dmg",
        _ if host.appimage => "appimage",
        _ => "deb",
    }
}

fn plan(host: &Host, release: &ReleaseInfo) -> (String, String, bool, bool, String) {
    if host.dev {
        return ("page".into(), String::new(), false, false, "dev".into());
    }
    let mode = preferred_mode(host);
    match find_asset(&release.assets, mode, host.arch) {
        Some(name) => {
            let quitting = matches!(mode, "setup" | "portable" | "appimage");
            (mode.into(), name.to_string(), true, quitting, String::new())
        }
        None => ("page".into(), String::new(), false, false, "no_asset".into()),
    }
}

fn none_available(current: &str) -> UpdateCheck {
    UpdateCheck {
        current: current.to_string(),
        latest: current.to_string(),
        available: false,
        prerelease: false,
        notes: String::new(),
        html_url: latest_page(),
        asset_name: String::new(),
        mode: String::new(),
        installable: false,
        quitting: false,
        reason: String::new(),
        tag: String::new(),
    }
}

fn resolve(current_raw: &str, releases: &[ReleaseInfo], host: &Host) -> Result<UpdateCheck, String> {
    let current_ver = parse_semver(current_raw).ok_or_else(|| tr("err.update.bad_version"))?;
    let current = version_text(current_raw);
    let include_pre = !current_ver.pre.is_empty();
    let best = releases
        .iter()
        .filter(|r| !r.draft && r.version > current_ver && (include_pre || !r.prerelease))
        .max_by(|a, b| a.version.cmp(&b.version));
    let Some(rel) = best else {
        return Ok(none_available(&current));
    };
    let (mode, asset_name, installable, quitting, reason) = plan(host, rel);
    Ok(UpdateCheck {
        current,
        latest: rel.version_text.clone(),
        available: true,
        prerelease: rel.prerelease,
        notes: rel.notes.clone(),
        html_url: releases_page(&rel.tag),
        asset_name,
        mode,
        installable,
        quitting,
        reason,
        tag: rel.tag.clone(),
    })
}

pub fn check(current: &str, exe: Option<&Path>) -> Result<UpdateCheck, String> {
    let releases = fetch_releases()?;
    resolve(current, &releases, &detect_host(exe))
}

pub fn install(app: AppHandle, current: &str, exe: &Path) -> Result<UpdateApply, String> {
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return Err(tr("err.update.busy"));
    }
    let _guard = InstallGuard;
    let host = detect_host(Some(exe));
    let releases = fetch_releases()?;
    let info = resolve(current, &releases, &host)?;
    if !info.available {
        return Err(tr("err.update.none"));
    }
    if !info.installable {
        crate::store::open_url(&info.html_url)?;
        return Ok(UpdateApply { mode: "page".into(), quitting: false });
    }
    let rel = releases.iter().find(|r| r.tag == info.tag).ok_or_else(|| tr("err.update.none"))?;
    let dest = download_verified(&app, rel, &info.asset_name)?;
    apply(&app, &info, &dest, exe)
}

fn apply(app: &AppHandle, info: &UpdateCheck, dest: &Path, exe: &Path) -> Result<UpdateApply, String> {
    emit_progress(app, "apply", 0, 0);
    if dest == exe {
        let _ = fs::remove_file(dest);
        return Err(tr("err.update.path"));
    }
    match info.mode.as_str() {
        #[cfg(windows)]
        "setup" => apply_setup(app, dest),
        #[cfg(windows)]
        "portable" => apply_portable(app, dest, exe),
        #[cfg(not(windows))]
        "setup" | "portable" => Err(tr("err.update.no_asset")),
        #[cfg(unix)]
        "appimage" => apply_appimage(app, dest),
        #[cfg(not(unix))]
        "appimage" => Err(tr("err.update.no_asset")),
        #[cfg(not(windows))]
        "dmg" | "deb" => {
            open_path(dest)?;
            Ok(UpdateApply { mode: info.mode.clone(), quitting: false })
        }
        #[cfg(windows)]
        "dmg" | "deb" => Err(tr("err.update.no_asset")),
        "page" => {
            crate::store::open_url(&info.html_url)?;
            Ok(UpdateApply { mode: "page".into(), quitting: false })
        }
        _ => Err(tr("err.update.no_asset")),
    }
}

fn download_verified(app: &AppHandle, rel: &ReleaseInfo, asset_name: &str) -> Result<PathBuf, String> {
    if !safe_token(asset_name) {
        return Err(tr("err.update.path"));
    }
    emit_progress(app, "checksum", 0, 0);
    let checksum_names: Vec<&String> = rel.assets.iter().filter(|n| is_checksum_name(n)).collect();
    if checksum_names.is_empty() {
        return Err(tr("err.update.checksum_missing"));
    }
    let mut entries = Vec::new();
    let mut last_err: Option<String> = None;
    for name in checksum_names {
        let url = download_url(&rel.tag, name)?;
        let resp = match call(&file_agent(), &url, "text/plain,*/*") {
            Ok(r) => r,
            Err(e) => {
                last_err = Some(e);
                continue;
            }
        };
        match read_capped(resp, MAX_CHECKSUM) {
            Ok(bytes) => entries.extend(parse_checksums(&String::from_utf8_lossy(&bytes))),
            Err(e) => last_err = Some(e),
        }
    }
    if entries.is_empty() {
        return Err(last_err.unwrap_or_else(|| tr("err.update.checksum_missing")));
    }
    let expected = match lookup_hash(&entries, asset_name) {
        Ok(Some(h)) => h,
        Ok(None) => return Err(tr("err.update.checksum_missing")),
        Err(()) => return Err(tr("err.update.checksum_mismatch")),
    };

    let url = download_url(&rel.tag, asset_name)?;
    let dir = std::env::temp_dir().join("zcode-switch-update");
    fs::create_dir_all(&dir).map_err(|e| trf("err.update.write", &[("e", &clip(&e.to_string()))]))?;
    let dest = dir.join(asset_name);
    let hashed = download_hashed(&file_agent(), &url, &dest, MAX_INSTALLER, app, "download");
    let got = match hashed {
        Ok(h) => h,
        Err(e) => {
            let _ = fs::remove_file(&dest);
            return Err(e);
        }
    };
    emit_progress(app, "verify", 0, 0);
    if !got.eq_ignore_ascii_case(&expected) {
        let _ = fs::remove_file(&dest);
        return Err(tr("err.update.checksum_mismatch"));
    }
    Ok(dest)
}

fn is_checksum_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("checksums-") && n.ends_with(".txt")
}

fn parse_checksums(text: &str) -> Vec<(String, String)> {
    text.lines().filter_map(parse_checksum_line).collect()
}

fn parse_checksum_line(line: &str) -> Option<(String, String)> {
    let line = line.trim().trim_start_matches('\u{feff}');
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let mut parts = line.split_whitespace();
    let hash = parts.next()?.to_ascii_lowercase();
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let name = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let name = name.trim_start_matches('*');
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    if !safe_token(name) {
        return None;
    }
    Some((hash, name.to_string()))
}

fn lookup_hash(entries: &[(String, String)], name: &str) -> Result<Option<String>, ()> {
    let mut found: Option<String> = None;
    for (hash, fname) in entries {
        if fname.eq_ignore_ascii_case(name) {
            if found.as_ref().is_some_and(|h| h != hash) {
                return Err(());
            }
            found = Some(hash.clone());
        }
    }
    Ok(found)
}

fn api_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout(Duration::from_secs(20))
        .redirects(5)
        .build()
}

fn file_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout(Duration::from_secs(180))
        .redirects(8)
        .build()
}

fn map_http(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, _) if code == 403 || code == 429 => tr("err.update.rate"),
        ureq::Error::Status(code, _) => trf("err.update.http", &[("code", &code.to_string())]),
        other => trf("err.update.network", &[("e", &clip(&other.to_string()))]),
    }
}

fn call(agent: &ureq::Agent, url: &str, accept: &str) -> Result<ureq::Response, String> {
    agent
        .get(url)
        .set("User-Agent", UA)
        .set("Accept", accept)
        .set("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(map_http)
}

fn fetch_releases() -> Result<Vec<ReleaseInfo>, String> {
    let resp = call(&api_agent(), RELEASES_URL, "application/vnd.github+json")?;
    let bytes = read_capped(resp, MAX_API)?;
    parse_releases(&String::from_utf8_lossy(&bytes))
}

fn read_capped(resp: ureq::Response, max: u64) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let mut reader = resp.into_reader();
    let mut chunk = [0u8; 8192];
    loop {
        let n = reader.read(&mut chunk).map_err(|e| trf("err.update.network", &[("e", &clip(&e.to_string()))]))?;
        if n == 0 {
            break;
        }
        if buf.len() as u64 + n as u64 > max {
            return Err(tr("err.update.too_big"));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(buf)
}

fn download_hashed(agent: &ureq::Agent, url: &str, dest: &Path, max: u64, app: &AppHandle, phase: &str) -> Result<String, String> {
    let resp = call(agent, url, "application/octet-stream")?;
    let total = resp.header("content-length").and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    if total > max {
        return Err(tr("err.update.too_big"));
    }
    let mut file = File::create(dest).map_err(|e| trf("err.update.write", &[("e", &clip(&e.to_string()))]))?;
    let mut hasher = Sha256::new();
    let mut reader = resp.into_reader();
    let mut buf = [0u8; 64 * 1024];
    let mut received = 0u64;
    let mut last_emit = 0u64;
    emit_progress(app, phase, 0, total);
    loop {
        let n = reader.read(&mut buf).map_err(|e| trf("err.update.download", &[("e", &clip(&e.to_string()))]))?;
        if n == 0 {
            break;
        }
        received += n as u64;
        if received > max {
            return Err(tr("err.update.too_big"));
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| trf("err.update.write", &[("e", &clip(&e.to_string()))]))?;
        if received - last_emit >= 256 * 1024 || (total > 0 && received == total) {
            last_emit = received;
            emit_progress(app, phase, received, total);
        }
    }
    file.sync_all().map_err(|e| trf("err.update.write", &[("e", &clip(&e.to_string()))]))?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn emit_progress(app: &AppHandle, phase: &str, received: u64, total: u64) {
    let _ = app.emit("update-progress", serde_json::json!({
        "phase": phase,
        "received": received,
        "total": total,
    }));
}

#[cfg(windows)]
fn spawn_detached(cmd: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    const BASE: u32 = 0x0000_0008 | 0x0000_0200 | 0x0800_0000;
    const BREAKAWAY: u32 = 0x0100_0000;
    cmd.creation_flags(BASE | BREAKAWAY);
    match cmd.spawn() {
        Ok(child) => Ok(child),
        Err(_) => {
            cmd.creation_flags(BASE);
            cmd.spawn()
        }
    }
}

#[cfg(not(windows))]
fn spawn_detached(cmd: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd.process_group(0);
    cmd.spawn()
}

#[cfg(windows)]
fn apply_setup(app: &AppHandle, dest: &Path) -> Result<UpdateApply, String> {
    let mut cmd = std::process::Command::new(dest);
    spawn_detached(&mut cmd).map_err(|e| trf("err.update.apply", &[("e", &clip(&e.to_string()))]))?;
    exit_soon(app);
    Ok(UpdateApply { mode: "setup".into(), quitting: true })
}

#[cfg(windows)]
fn apply_portable(app: &AppHandle, dest: &Path, exe: &Path) -> Result<UpdateApply, String> {
    plain_path(exe)?;
    plain_path(dest)?;
    let src = ps_literal(dest)?;
    let dst = ps_literal(exe)?;
    let body = format!(
        "$ErrorActionPreference = 'Stop'\n\
         $src = {src}\n\
         $dst = {dst}\n\
         for ($i = 0; $i -lt 40; $i++) {{\n\
           Start-Sleep -Milliseconds 500\n\
           try {{\n\
             Copy-Item -LiteralPath $src -Destination $dst -Force\n\
             Start-Process -FilePath $dst\n\
             Remove-Item -LiteralPath $src -Force -ErrorAction SilentlyContinue\n\
             exit 0\n\
           }} catch {{}}\n\
         }}\n\
         exit 1\n"
    );
    let script = dest.parent().unwrap_or(Path::new(".")).join("zcode-switch-update.ps1");
    write_ps1(&script, &body)?;
    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-ExecutionPolicy", "Bypass", "-File"]);
    cmd.arg(&script);
    spawn_detached(&mut cmd).map_err(|e| trf("err.update.apply", &[("e", &clip(&e.to_string()))]))?;
    exit_soon(app);
    Ok(UpdateApply { mode: "portable".into(), quitting: true })
}

#[cfg(windows)]
fn write_ps1(path: &Path, body: &str) -> Result<(), String> {
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend(body.as_bytes());
    fs::write(path, bytes).map_err(|e| trf("err.update.write", &[("e", &clip(&e.to_string()))]))
}

#[cfg(windows)]
fn ps_literal(p: &Path) -> Result<String, String> {
    let s = p.to_str().filter(|s| !s.is_empty() && !s.contains('\n') && !s.contains('\r') && !s.contains('\0')).ok_or_else(|| tr("err.update.path"))?;
    Ok(format!("'{}'", s.replace('\'', "''")))
}

fn exit_soon(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        app.exit(0);
    });
}

fn plain_path(p: &Path) -> Result<(), String> {
    let s = p.to_str().ok_or_else(|| tr("err.update.path"))?;
    if s.is_empty() || s.contains('\n') || s.contains('\r') || s.contains('\0') || !p.is_absolute() {
        return Err(tr("err.update.path"));
    }
    Ok(())
}

#[cfg(unix)]
fn apply_appimage(app: &AppHandle, dest: &Path) -> Result<UpdateApply, String> {
    use std::os::unix::fs::PermissionsExt;
    let target = appimage_path()?;
    if dest == target {
        return Err(tr("err.update.path"));
    }
    fs::set_permissions(dest, fs::Permissions::from_mode(0o755)).map_err(|e| trf("err.update.write", &[("e", &clip(&e.to_string()))]))?;
    fs::copy(dest, &target).map_err(|e| trf("err.update.write", &[("e", &clip(&e.to_string()))]))?;
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).map_err(|e| trf("err.update.write", &[("e", &clip(&e.to_string()))]))?;
    let quoted = sh_quote(&target)?;
    let script_path = dest.parent().unwrap_or(Path::new(".")).join("zcode-switch-update.sh");
    let body = format!("#!/bin/sh\nsleep 2\nexec {quoted}\n");
    fs::write(&script_path, body).map_err(|e| trf("err.update.write", &[("e", &clip(&e.to_string()))]))?;
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).map_err(|e| trf("err.update.write", &[("e", &clip(&e.to_string()))]))?;
    let mut cmd = std::process::Command::new(&script_path);
    spawn_detached(&mut cmd).map_err(|e| trf("err.update.apply", &[("e", &clip(&e.to_string()))]))?;
    exit_soon(app);
    Ok(UpdateApply { mode: "appimage".into(), quitting: true })
}

#[cfg(unix)]
fn sh_quote(p: &Path) -> Result<String, String> {
    let s = p.to_str().filter(|s| !s.is_empty() && !s.contains('\n') && !s.contains('\r') && !s.contains('\0')).ok_or_else(|| tr("err.update.path"))?;
    Ok(format!("'{}'", s.replace('\'', "'\\''")))
}

#[cfg(unix)]
fn appimage_path() -> Result<PathBuf, String> {
    let raw = std::env::var("APPIMAGE").map_err(|_| tr("err.update.path"))?;
    let path = PathBuf::from(&raw);
    plain_path(&path)?;
    Ok(path)
}

#[cfg(not(windows))]
fn open_path(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(not(target_os = "macos"))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(path);
    spawn_detached(&mut cmd).map(|_| ()).map_err(|e| trf("err.update.apply", &[("e", &clip(&e.to_string()))]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(os: &'static str, arch: &'static str) -> Host {
        Host { os, arch, portable: false, appimage: false, dev: false }
    }

    fn rel(tag: &str, pre: bool, assets: &[&str]) -> ReleaseInfo {
        ReleaseInfo {
            version_text: version_text(tag),
            version: parse_semver(tag).unwrap(),
            tag: tag.to_string(),
            prerelease: pre,
            draft: false,
            notes: String::new(),
            assets: assets.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn semver_orders_prerelease_and_numeric_ids() {
        let v = |s| parse_semver(s).unwrap();
        assert!(v("1.6.0") < v("1.7.0"));
        assert!(v("1.9.0") < v("1.10.0"));
        assert!(v("1.7.0-beta.1") < v("1.7.0-beta.2"));
        assert!(v("1.7.0-beta.2") < v("1.7.0-beta.10"));
        assert!(v("1.7.0-beta.1") < v("1.7.0"));
        assert_eq!(v("v1.6.0"), v("1.6.0"));
        assert!(parse_semver("1.7").is_none());
        assert!(parse_semver("1.7.0-beta.01").is_none());
    }

    #[test]
    fn stable_channel_skips_newer_prerelease() {
        let releases = vec![
            rel("v1.6.0", false, &["zcode-switch_1.6.0_x64-setup.exe"]),
            rel("v1.7.0-beta.1", true, &["zcode-switch_1.7.0-beta.1_x64-setup.exe"]),
        ];
        let info = resolve("1.6.0", &releases, &host("windows", "x86_64")).unwrap();
        assert!(!info.available);

        let info = resolve("1.5.0", &releases, &host("windows", "x86_64")).unwrap();
        assert!(info.available);
        assert_eq!(info.latest, "1.6.0");
        assert_eq!(info.mode, "setup");
        assert!(info.installable);
        assert!(info.quitting);
        assert_eq!(info.asset_name, "zcode-switch_1.6.0_x64-setup.exe");
    }

    #[test]
    fn beta_channel_can_move_to_newer_beta_or_stable() {
        let releases = vec![
            rel("v1.6.0", false, &["zcode-switch_1.6.0_x64-setup.exe"]),
            rel("v1.7.0-beta.2", true, &["zcode-switch_1.7.0-beta.2_x64-setup.exe"]),
        ];
        let info = resolve("1.7.0-beta.1", &releases, &host("windows", "x86_64")).unwrap();
        assert_eq!(info.latest, "1.7.0-beta.2");
        assert!(info.prerelease);

        let releases = vec![rel("v1.7.0", false, &["zcode-switch_1.7.0_x64-setup.exe", "zcode-switch_1.7.0_portable.exe"])];
        let mut portable = host("windows", "x86_64");
        portable.portable = true;
        let info = resolve("1.7.0-beta.1", &releases, &portable).unwrap();
        assert_eq!(info.latest, "1.7.0");
        assert_eq!(info.mode, "portable");
        assert_eq!(info.asset_name, "zcode-switch_1.7.0_portable.exe");
    }

    #[test]
    fn draft_is_ignored_and_dev_build_opens_the_release_page() {
        let mut draft = rel("v9.0.0", false, &["zcode-switch_9.0.0_x64-setup.exe"]);
        draft.draft = true;
        let info = resolve("1.0.0", &[draft], &host("windows", "x86_64")).unwrap();
        assert!(!info.available);

        let mut dev = host("windows", "x86_64");
        dev.dev = true;
        let info = resolve("1.0.0", &[rel("v1.2.0", false, &["zcode-switch_1.2.0_x64-setup.exe"])], &dev).unwrap();
        assert!(info.available);
        assert!(!info.installable);
        assert_eq!(info.mode, "page");
        assert_eq!(info.reason, "dev");
        assert_eq!(info.html_url, "https://github.com/pjpv/zcode-switch/releases/tag/v1.2.0");
    }

    #[test]
    fn picks_platform_asset_or_falls_back_to_the_page() {
        let assets = [
            "zcode-switch_1.6.0_x64-setup.exe",
            "zcode-switch_1.6.0_portable.exe",
            "zcode-switch_1.6.0_aarch64.dmg",
            "zcode-switch_1.6.0_x64.dmg",
            "zcode-switch_1.6.0_amd64.deb",
            "zcode-switch_1.6.0_amd64.AppImage",
        ];
        let rel = rel("v1.6.0", false, &assets);
        assert_eq!(find_asset(&rel.assets, "dmg", "aarch64"), Some("zcode-switch_1.6.0_aarch64.dmg"));
        assert_eq!(find_asset(&rel.assets, "dmg", "x86_64"), Some("zcode-switch_1.6.0_x64.dmg"));
        assert_eq!(find_asset(&rel.assets, "deb", "aarch64"), None);

        let mut linux = host("linux", "x86_64");
        linux.appimage = true;
        let info = resolve("1.0.0", &[rel.clone()], &linux).unwrap();
        assert_eq!(info.mode, "appimage");
        assert!(info.quitting);

        let info = resolve("1.0.0", &[rel], &host("linux", "aarch64")).unwrap();
        assert!(info.available);
        assert_eq!(info.reason, "no_asset");
        assert!(!info.installable);
    }

    #[test]
    fn parse_drops_unsafe_names_and_reads_checksums() {
        let raw = r#"[{
            "tag_name":"v1.2.0",
            "prerelease":false,
            "draft":false,
            "body":"hello",
            "assets":[
                {"name":"../evil.exe","browser_download_url":"https://evil.example/x"},
                {"name":"zcode-switch_1.2.0_x64-setup.exe"},
                {"name":"checksums-Windows-v1.2.0.txt"}
            ]
        }]"#;
        let rels = parse_releases(raw).unwrap();
        assert_eq!(rels[0].assets, vec!["zcode-switch_1.2.0_x64-setup.exe".to_string(), "checksums-Windows-v1.2.0.txt".to_string()]);
        let url = download_url(&rels[0].tag, &rels[0].assets[0]).unwrap();
        assert_eq!(url, "https://github.com/pjpv/zcode-switch/releases/download/v1.2.0/zcode-switch_1.2.0_x64-setup.exe");
        assert!(download_url("v1.2.0", "../evil.exe").is_err());

        let text = "\
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  zcode-switch_1.2.0_x64-setup.exe\n\
fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210 *other.exe\n";
        let entries = parse_checksums(text);
        assert_eq!(lookup_hash(&entries, "zcode-switch_1.2.0_x64-setup.exe").unwrap().unwrap(), "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");

        let conflict = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  a.exe\n\
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  a.exe\n";
        assert!(lookup_hash(&parse_checksums(conflict), "a.exe").is_err());
    }
}
