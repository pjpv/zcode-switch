
use std::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
}

impl Lang {
    pub fn as_str(self) -> &'static str {
        match self {
            Lang::Zh => "zh",
            Lang::En => "en",
        }
    }
    pub fn locale_tag(self) -> &'static str {
        match self {
            Lang::Zh => "zh-CN",
            Lang::En => "en",
        }
    }
    pub fn parse(s: &str) -> Option<Lang> {
        match s.trim().to_ascii_lowercase().as_str() {
            "zh" | "zh-cn" | "zh_cn" | "zh-hans" => Some(Lang::Zh),
            "en" | "en-us" | "en-gb" | "en-us-posix" => Some(Lang::En),
            _ => None,
        }
    }
}

static LANG: RwLock<Lang> = RwLock::new(Lang::Zh);

fn lang_cell() -> std::sync::RwLockReadGuard<'static, Lang> {
    LANG.read().unwrap_or_else(|e| e.into_inner())
}

pub fn current() -> Lang {
    *lang_cell()
}

pub fn set(lang: Lang) {
    *LANG.write().unwrap_or_else(|e| e.into_inner()) = lang;
}

pub fn resolve(explicit: Option<&str>) -> Lang {
    if let Some(l) = explicit.and_then(Lang::parse) {
        return l;
    }
    if let Some(os) = sys_locale::get_locale() {
        if !os.to_ascii_lowercase().starts_with("zh") {
            return Lang::En;
        }
    }
    Lang::Zh
}

pub fn init_from_settings(s: &crate::store::Settings) {
    set(resolve(s.language.as_deref()));
}

const ZH: &[(&str, &str)] = &[
    ("err.write", "写入失败：{e}"),
    ("err.write_file", "写入失败 {path}: {e}"),
    ("err.rename_fail", "落盘失败 {path}: {e}"),
    ("err.read", "无法读取 {path}: {e}"),
    ("err.bad_json", "{path} 不是有效的 JSON：{e}"),
    ("err.mkdir", "无法创建目录：{e}"),
    ("err.serialize", "序列化失败：{e}"),
    ("err.store.mk_accounts_dir", "无法创建账号库目录：{e}"),
    ("err.store.not_object", "credentials.json 内容不是对象"),
    ("err.store.list_fail", "读取账号库失败：{e}"),
    ("err.store.bad_id", "非法的账号 id"),
    ("err.store.no_account_id", "账号不存在：{id}"),
    ("err.store.corrupt", "账号存档损坏：{e}"),
    ("err.store.no_account", "账号不存在"),
    ("err.store.delete_fail", "删除失败：{e}"),
    ("err.name.empty", "名称不能为空"),
    ("err.name.too_long", "名称过长（最多 40 字符）"),
    ("err.name.taken", "名称「{name}」已被账号「{other}」占用"),
    ("err.live.no_creds_file", "当前没有 credentials.json，请先在 ZCode 里登录"),
    ("err.live.no_credentials", "当前文件里没有登录凭据（未登录）"),
    ("err.live.dup_saved", "当前登录已保存为「{name}」，无需重复保存"),
    ("err.live.no_file", "当前没有登录文件"),
    ("err.live.logged_out", "当前未登录"),
    ("err.live.same", "当前登录与「{name}」一致，请直接切换"),
    ("err.live.quota", "当前未登录，无法查询额度"),
    ("err.switch.running", "ZCode 正在运行，请先完全退出（含托盘），或使用强制切换（自动关闭并重启）"),
    ("err.switch.kill_timeout", "关闭 ZCode 超时，已取消切换（避免登录态损坏）"),
    ("err.write_config", "写入 config 失败：{e}"),
    ("err.hot.verify", "热切换校验失败：ZCode 并发回写冲突，已重试 3 次。建议改用重启式切换"),
    ("err.zcode.missing", "ZCode 不存在：{path}（在设置里修改路径）"),
    ("err.zcode.launch", "启动失败：{e}"),
    ("err.zcode.path_invalid", "ZCode 路径无效：{p}"),
    ("err.zcode.path_invalid_hint", "ZCode 路径无效：{p}（在设置里修改）"),
    ("err.zcode.kill_timeout", "关闭 ZCode 超时"),
    ("err.cipher.pw_empty", "密码不能为空（导出文件包含登录凭据，必须加密）"),
    ("err.cipher.key", "密钥错误：{e}"),
    ("err.cipher.encrypt", "加密失败：{e}"),
    ("err.cipher.no_kdf", "文件缺少 kdf 段（不是有效的加密导出）"),
    ("err.cipher.no_cipher", "文件缺少 cipher 段"),
    ("err.cipher.bad_salt", "salt 解码失败"),
    ("err.cipher.bad_nonce", "nonce 解码失败"),
    ("err.cipher.bad_tag", "tag 解码失败"),
    ("err.cipher.bad_data", "数据解码失败"),
    ("err.cipher.nonce_len", "nonce 长度异常"),
    ("err.cipher.wrong_pw", "密码错误或文件已损坏"),
    ("err.cipher.bad_plain", "解密后内容异常：{e}"),
    ("err.bundle.no_accounts", "捆绑包缺少 accounts 数组"),
    ("err.bundle.no_creds", "捆绑包条目缺少 credentials"),
    ("err.bundle.unrecognized", "无法识别的文件格式（仅支持本工具导出的加密捆绑包 .zsb）"),
    ("err.import.no_creds", "{fname}：无登录凭据"),
    ("err.import.wrap", "{fname}：{e}"),
    ("err.import.dup", "{fname}：已存在于账号库"),
    ("err.import.read", "{fname}：读取失败 {e}"),
    ("err.import.not_sealed", "{fname}：不是加密捆绑包（仅支持本工具导出的 .zsb）"),
    ("err.import.not_bundle", "{fname}：不是捆绑包格式（仅支持导出全部生成的 .zsb）"),
    ("err.import.json", "{fname}：JSON 解析失败 {e}"),
    ("err.import.not_sealed_plain", "不是加密捆绑包（仅支持本工具导出的 .zsb）"),
    ("err.import.not_bundle_plain", "不是捆绑包格式（仅支持导出全部生成的 .zsb）"),
    ("err.export.empty", "账号库为空，没有可导出的内容"),
    ("err.export.empty_short", "账号库为空"),
    ("tray.show", "显示主窗口"),
    ("tray.capture", "保存当前登录"),
    ("tray.launch", "启动 ZCode"),
    ("tray.kill", "关闭 ZCode"),
    ("tray.quit", "退出"),
    ("tray.unsaved", "未保存的登录"),
    ("tray.logged_out", "未登录"),
    ("title.login", "登录 ZCode 账号"),
    ("login.assist.email", "邮箱登录"),
    ("login.assist.signup", "邮箱注册"),
    ("login.assist.hint", "已在邮箱登录页"),
    ("login.assist.hint_signup", "已在邮箱注册页"),
    ("title.captcha", "安全验证"),
    ("title.settings", "Z·SWITCH 设置"),
    ("dialog.zsb", "ZSwitch 加密捆绑包（.zsb）"),
    ("dialog.exe", "ZCode 可执行文件"),
    ("err.main.missing", "主窗口不存在"),
    ("err.path.invalid", "路径无效：{e}"),
    ("err.path.conv", "路径错误：{e}"),
    ("err.oauth.unknown_provider", "未知登录提供方：{provider}"),
    ("err.proxy.invalid", "代理地址无效：{e}"),
    ("err.oauth.appdata", "无法定位应用数据目录：{e}"),
    ("err.oauth.bad_authorize_url", "authorize URL 非法：{e}"),
    ("err.oauth.window", "登录窗口创建失败：{e}"),
    ("err.proxy.need_url", "开启代理前请先填写代理地址（http:// 或 socks5://）"),
    ("err.oauth.state", "OAuth state 校验失败，请重新发起登录"),
    ("err.oauth.flow", "登录流程异常：{e}"),
    ("err.oauth.not_callback", "不是 OAuth 回调地址"),
    ("err.oauth.bad_cb", "回调参数格式错误"),
    ("err.oauth.no_code_state", "回调缺少 code 或 state"),
    ("err.oauth.init", "OAuth flow 初始化失败：{e}"),
    ("err.oauth.init_invalid", "OAuth flow 初始化响应无效"),
    ("err.oauth.init_invalid_msg", "OAuth flow 初始化失败：{msg}"),
    ("err.oauth.flow_failed", "OAuth flow 授权失败"),
    ("err.oauth.poll_invalid", "OAuth flow 查询响应无效"),
    ("err.oauth.poll_invalid_msg", "OAuth flow 查询失败：{msg}"),
    ("err.oauth.poll_terminal", "OAuth flow 已失效（HTTP {code}）"),
    ("err.oauth.zai_business", "z.ai 业务令牌换取失败，请重新登录"),
    ("err.oauth.expired", "OAuth 登录流程已过期，请重新发起"),
    ("err.oauth.exchange_req", "token 交换请求失败：{e}"),
    ("err.oauth.exchange", "token 交换失败（{code}）：{msg}"),
    ("err.oauth.no_token", "token 交换成功但响应缺少 token 字段"),
    ("err.proxy.empty", "代理地址不能为空"),
    ("err.proxy.scheme", "地址需以 http:// 或 socks5:// 开头（如 http://127.0.0.1:7890）"),
    ("err.proxy.no_auth", "代理不支持账号密码认证，请使用免认证的本地代理"),
    ("err.proxy.no_path", "代理地址不包含路径，只需 scheme://主机:端口"),
    ("err.proxy.need_port", "代理地址必须带端口（如 :7890）"),
    ("err.proxy.empty_host", "代理主机不能为空"),
    ("err.proxy.bad_host", "代理主机格式不正确"),
    ("err.proxy.port_nan", "端口“{port}”不是数字"),
    ("err.proxy.port_range", "端口 {port} 超出范围（1-65535）"),
    ("err.http.read", "读取响应失败：{e}"),
    ("err.token.biz401", "Token 已过期或无效（业务码 401）"),
    ("err.quota.rate_limited", "服务端限流，正在重试..."),
    ("err.quota.http429", "额度接口 HTTP 429"),
    ("err.token.http401", "Token 已过期或无效（HTTP {code}）"),
    ("err.quota.http", "额度接口 HTTP {code}: {msg}"),
    ("err.network", "网络请求失败：{e}"),
    ("err.quota.biz", "业务码 {code}: {msg}"),
    ("err.quota.bad_resp", "额度接口返回异常"),
    ("err.quota.fail", "额度查询失败"),
    ("err.quota.no_token", "未找到可用于查询额度的 ZCode token，请先登录或切换账号"),
    ("err.token.expired", "该账号 Token 已过期，请删除后重新登录"),
    ("probe.no_entitlement", "服务端该账号当前没有可用套餐/模型（可能试用已耗尽或被风控），ZCode 里模型列表会为空"),
    ("probe.auth_failed", "该账号 token 已被服务端拒绝，请在 ZCode 内重新登录此账号后再切换"),
    ("err.claim.no_jwt", "该账号缺少 zcodejwttoken 凭证，请先在 ZCode 客户端登录一次刷新"),
    ("err.claim.preview_req", "preview 请求失败"),
    ("err.claim.claim_req", "领取请求失败"),
    ("err.claim.no_captcha", "验证码参数为空，请重试"),
    ("err.claim.config_req", "配置请求失败：{e}"),
    ("err.claim.config_unavailable", "验证码配置不可用"),
    ("err.claim.gone", "该套餐已不可领取，请刷新"),
    ("err.claim.none_pending", "没有待领取的套餐"),
    ("err.claim.activate_req", "激活上报失败：{e}"),
    ("claim.fail.1001", "套餐不存在"),
    ("claim.fail.1002", "活动已结束或套餐暂不可领取"),
    ("claim.fail.1003", "该套餐已经领取过"),
    ("claim.fail.1004", "不符合领取条件"),
    ("claim.fail.1005", "今日领取名额已用完"),
    ("claim.fail.3001", "领取参数错误，请刷新后重试"),
    ("claim.fail.3007", "验证码校验失败，请重试"),
    ("claim.fail.401", "请先登录后再领取"),
    ("claim.fail.generic", "领取失败"),
    ("claim.fail.with_server", "{base}（{server_msg}）"),
    ("cli.pw_hint", "缺少密码：用环境变量 ZSW_PASSWORD（推荐，不会出现在进程列表/命令历史）或 --password <密码>"),
    ("cli.missing_cmd", "缺少子命令：state|list|capture|rename|delete|update|switch|quota|claim-preview|kill|export|export-all|import|behavior|setpath|launch"),
    ("cli.usage.rename", "用法：rename --id <id> --name <名称>"),
    ("cli.usage.delete", "用法：delete --id <id>"),
    ("cli.usage.update", "用法：update --id <id>"),
    ("cli.usage.switch", "用法：switch --id <id> [--force] [--restart|--no-restart]"),
    ("cli.usage.export", "用法：export --id <id> --out <file.zsb>（密码：ZSW_PASSWORD 或 --password）"),
    ("cli.usage.export_all", "用法：export-all --out <file.zsb>（密码：ZSW_PASSWORD 或 --password）"),
    ("cli.usage.import", "用法：import --file <file.zsb>（密码：ZSW_PASSWORD 或 --password）"),
    ("cli.usage.setpath", "用法：setpath --path <ZCode.exe>"),
    ("cli.unknown_cmd", "未知子命令：{cmd}"),
    ("cli.read_fail", "读取失败：{e}"),
    ("cli.json_fail", "JSON 解析失败：{e}"),
    ("err.lang.unknown", "未知语言：{lang}（支持 zh / en）"),
];

const EN: &[(&str, &str)] = &[
    ("err.write", "Write failed: {e}"),
    ("err.write_file", "Write failed {path}: {e}"),
    ("err.rename_fail", "Persist failed {path}: {e}"),
    ("err.read", "Cannot read {path}: {e}"),
    ("err.bad_json", "{path} is not valid JSON: {e}"),
    ("err.mkdir", "Cannot create directory: {e}"),
    ("err.serialize", "Serialization failed: {e}"),
    ("err.store.mk_accounts_dir", "Cannot create the account store directory: {e}"),
    ("err.store.not_object", "credentials.json is not an object"),
    ("err.store.list_fail", "Failed to read the account store: {e}"),
    ("err.store.bad_id", "Invalid account id"),
    ("err.store.no_account_id", "Account not found: {id}"),
    ("err.store.corrupt", "Account archive corrupted: {e}"),
    ("err.store.no_account", "Account not found"),
    ("err.store.delete_fail", "Delete failed: {e}"),
    ("err.name.empty", "Name cannot be empty"),
    ("err.name.too_long", "Name too long (max 40 characters)"),
    ("err.name.taken", "Name \"{name}\" is already used by account \"{other}\""),
    ("err.live.no_creds_file", "No credentials.json present — log in inside ZCode first"),
    ("err.live.no_credentials", "Current file has no login credentials (not logged in)"),
    ("err.live.dup_saved", "Current login is already saved as \"{name}\""),
    ("err.live.no_file", "No login file present"),
    ("err.live.logged_out", "Not logged in"),
    ("err.live.same", "Current login already matches \"{name}\" — just switch to it"),
    ("err.live.quota", "Not logged in — cannot query quota"),
    ("err.switch.running", "ZCode is running. Quit it fully (including the tray icon), or use Force switch (auto close & restart)"),
    ("err.switch.kill_timeout", "Timed out closing ZCode — switch cancelled to protect the login state"),
    ("err.write_config", "Failed to write config: {e}"),
    ("err.hot.verify", "Hot-switch verification failed: ZCode wrote back concurrently (3 retries). Prefer restart-style switching"),
    ("err.zcode.missing", "ZCode not found: {path} (change the path in Settings)"),
    ("err.zcode.launch", "Launch failed: {e}"),
    ("err.zcode.path_invalid", "Invalid ZCode path: {p}"),
    ("err.zcode.path_invalid_hint", "Invalid ZCode path: {p} (change it in Settings)"),
    ("err.zcode.kill_timeout", "Timed out closing ZCode"),
    ("err.cipher.pw_empty", "Password cannot be empty (exports contain login credentials and must be encrypted)"),
    ("err.cipher.key", "Key error: {e}"),
    ("err.cipher.encrypt", "Encryption failed: {e}"),
    ("err.cipher.no_kdf", "File is missing the kdf section (not a valid encrypted export)"),
    ("err.cipher.no_cipher", "File is missing the cipher section"),
    ("err.cipher.bad_salt", "salt decode failed"),
    ("err.cipher.bad_nonce", "nonce decode failed"),
    ("err.cipher.bad_tag", "tag decode failed"),
    ("err.cipher.bad_data", "data decode failed"),
    ("err.cipher.nonce_len", "abnormal nonce length"),
    ("err.cipher.wrong_pw", "Wrong password or corrupted file"),
    ("err.cipher.bad_plain", "Decrypted payload is invalid: {e}"),
    ("err.bundle.no_accounts", "Bundle is missing the accounts array"),
    ("err.bundle.no_creds", "Bundle entry is missing credentials"),
    ("err.bundle.unrecognized", "Unrecognized file format (only encrypted .zsb bundles exported by this tool are supported)"),
    ("err.import.no_creds", "{fname}: no login credentials"),
    ("err.import.wrap", "{fname}: {e}"),
    ("err.import.dup", "{fname}: already in the account store"),
    ("err.import.read", "{fname}: read failed {e}"),
    ("err.import.not_sealed", "{fname}: not an encrypted bundle (only .zsb exported by this tool)"),
    ("err.import.not_bundle", "{fname}: not a bundle (only .zsb generated by Export All)"),
    ("err.import.json", "{fname}: JSON parse failed {e}"),
    ("err.import.not_sealed_plain", "Not an encrypted bundle (only .zsb exported by this tool)"),
    ("err.import.not_bundle_plain", "Not a bundle (only .zsb generated by Export All)"),
    ("err.export.empty", "The account store is empty — nothing to export"),
    ("err.export.empty_short", "The account store is empty"),
    ("tray.show", "Show main window"),
    ("tray.capture", "Save current login"),
    ("tray.launch", "Launch ZCode"),
    ("tray.kill", "Quit ZCode"),
    ("tray.quit", "Exit"),
    ("tray.unsaved", "Unsaved login"),
    ("tray.logged_out", "Not logged in"),
    ("title.login", "Sign in to ZCode"),
    ("login.assist.email", "Email sign-in"),
    ("login.assist.signup", "Email sign-up"),
    ("login.assist.hint", "Already on the email sign-in page"),
    ("login.assist.hint_signup", "Already on the email sign-up page"),
    ("title.captcha", "Security verification"),
    ("title.settings", "Z·SWITCH Settings"),
    ("dialog.zsb", "ZSwitch encrypted bundle (.zsb)"),
    ("dialog.exe", "ZCode executable"),
    ("err.main.missing", "Main window not found"),
    ("err.path.invalid", "Invalid path: {e}"),
    ("err.path.conv", "Path error: {e}"),
    ("err.oauth.unknown_provider", "Unknown login provider: {provider}"),
    ("err.proxy.invalid", "Invalid proxy address: {e}"),
    ("err.oauth.appdata", "Cannot locate the app data directory: {e}"),
    ("err.oauth.bad_authorize_url", "Invalid authorize URL: {e}"),
    ("err.oauth.window", "Failed to create the login window: {e}"),
    ("err.proxy.need_url", "Enter a proxy address (http:// or socks5://) before enabling the proxy"),
    ("err.oauth.state", "OAuth state check failed — start the login again"),
    ("err.oauth.flow", "Login flow error: {e}"),
    ("err.oauth.not_callback", "Not an OAuth callback URL"),
    ("err.oauth.bad_cb", "Malformed callback parameters"),
    ("err.oauth.no_code_state", "Callback is missing code or state"),
    ("err.oauth.init", "OAuth flow init failed: {e}"),
    ("err.oauth.init_invalid", "Invalid OAuth flow init response"),
    ("err.oauth.init_invalid_msg", "OAuth flow init failed: {msg}"),
    ("err.oauth.flow_failed", "OAuth flow authorization failed"),
    ("err.oauth.poll_invalid", "Invalid OAuth flow poll response"),
    ("err.oauth.poll_invalid_msg", "OAuth flow poll failed: {msg}"),
    ("err.oauth.poll_terminal", "OAuth flow invalidated (HTTP {code})"),
    ("err.oauth.zai_business", "Failed to resolve the z.ai business token — log in again"),
    ("err.oauth.expired", "The OAuth login flow expired — start it again"),
    ("err.oauth.exchange_req", "Token exchange request failed: {e}"),
    ("err.oauth.exchange", "Token exchange failed ({code}): {msg}"),
    ("err.oauth.no_token", "Token exchange succeeded but the response has no token field"),
    ("err.proxy.empty", "Proxy address cannot be empty"),
    ("err.proxy.scheme", "Address must start with http:// or socks5:// (e.g. http://127.0.0.1:7890)"),
    ("err.proxy.no_auth", "Proxies with username/password auth are not supported — use a local proxy without auth"),
    ("err.proxy.no_path", "Proxy address takes no path, only scheme://host:port"),
    ("err.proxy.need_port", "Proxy address must include a port (e.g. :7890)"),
    ("err.proxy.empty_host", "Proxy host cannot be empty"),
    ("err.proxy.bad_host", "Malformed proxy host"),
    ("err.proxy.port_nan", "Port \"{port}\" is not a number"),
    ("err.proxy.port_range", "Port {port} out of range (1-65535)"),
    ("err.http.read", "Failed to read response: {e}"),
    ("err.token.biz401", "Token expired or invalid (business code 401)"),
    ("err.quota.rate_limited", "Server rate limit, retrying..."),
    ("err.quota.http429", "Quota API HTTP 429"),
    ("err.token.http401", "Token expired or invalid (HTTP {code})"),
    ("err.quota.http", "Quota API HTTP {code}: {msg}"),
    ("err.network", "Network request failed: {e}"),
    ("err.quota.biz", "Business code {code}: {msg}"),
    ("err.quota.bad_resp", "Quota API returned an unexpected response"),
    ("err.quota.fail", "Quota query failed"),
    ("err.quota.no_token", "No usable ZCode token found — log in or switch accounts first"),
    ("err.token.expired", "This account's token has expired — delete it and log in again"),
    ("probe.no_entitlement", "The server reports no usable plan/models for this account (trial exhausted or risk-controlled) — the model list in ZCode will be empty"),
    ("probe.auth_failed", "This account's token was rejected by the server — re-login this account in ZCode, then switch again"),
    ("err.claim.no_jwt", "This account has no zcodejwttoken credential — log in once in the ZCode client to refresh it"),
    ("err.claim.preview_req", "Preview request failed"),
    ("err.claim.claim_req", "Claim request failed"),
    ("err.claim.no_captcha", "Captcha parameter is empty — retry"),
    ("err.claim.config_req", "Config request failed: {e}"),
    ("err.claim.config_unavailable", "Captcha config unavailable"),
    ("err.claim.gone", "This plan is no longer claimable — refresh the list"),
    ("err.claim.none_pending", "No pending plan to claim"),
    ("err.claim.activate_req", "Activation report failed: {e}"),
    ("claim.fail.1001", "Plan does not exist"),
    ("claim.fail.1002", "The event has ended or the plan is not claimable yet"),
    ("claim.fail.1003", "This plan has already been claimed"),
    ("claim.fail.1004", "Not eligible for this plan"),
    ("claim.fail.1005", "Today's claim quota is used up"),
    ("claim.fail.3001", "Claim parameter error — refresh and retry"),
    ("claim.fail.3007", "Captcha verification failed — retry"),
    ("claim.fail.401", "Log in before claiming"),
    ("claim.fail.generic", "Claim failed"),
    ("claim.fail.with_server", "{base} ({server_msg})"),
    ("cli.pw_hint", "Missing password: set the ZSW_PASSWORD env var (recommended — never appears in process lists or shell history) or pass --password <password>"),
    ("cli.missing_cmd", "Missing subcommand: state|list|capture|rename|delete|update|switch|quota|claim-preview|kill|export|export-all|import|behavior|setpath|launch"),
    ("cli.usage.rename", "Usage: rename --id <id> --name <name>"),
    ("cli.usage.delete", "Usage: delete --id <id>"),
    ("cli.usage.update", "Usage: update --id <id>"),
    ("cli.usage.switch", "Usage: switch --id <id> [--force] [--restart|--no-restart]"),
    ("cli.usage.export", "Usage: export --id <id> --out <file.zsb> (password: ZSW_PASSWORD or --password)"),
    ("cli.usage.export_all", "Usage: export-all --out <file.zsb> (password: ZSW_PASSWORD or --password)"),
    ("cli.usage.import", "Usage: import --file <file.zsb> (password: ZSW_PASSWORD or --password)"),
    ("cli.usage.setpath", "Usage: setpath --path <ZCode.exe>"),
    ("cli.unknown_cmd", "Unknown subcommand: {cmd}"),
    ("cli.read_fail", "Read failed: {e}"),
    ("cli.json_fail", "JSON parse failed: {e}"),
    ("err.lang.unknown", "Unknown language: {lang} (supported: zh / en)"),
];

fn table_for(lang: Lang) -> &'static [(&'static str, &'static str)] {
    match lang {
        Lang::Zh => ZH,
        Lang::En => EN,
    }
}

fn lookup(table: &'static [(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

pub fn tr_lang(lang: Lang, key: &str) -> String {
    match lookup(table_for(lang), key).or_else(|| lookup(ZH, key)) {
        Some(s) => s.to_string(),
        None => {
            eprintln!("[i18n] missing key: {key}");
            key.to_string()
        }
    }
}

pub fn tr(key: &str) -> String {
    tr_lang(current(), key)
}

pub fn trf(key: &str, args: &[(&str, &str)]) -> String {
    let mut s = tr(key);
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

pub fn coded(code: &str, key: &str, args: &[(&str, &str)]) -> String {
    format!("{code}:{}", trf(key, args))
}

pub fn code_of(err: &str) -> Option<&str> {
    let (code, _) = err.split_once(':')?;
    (!code.is_empty()
        && !code.contains(' ')
        && code.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
        .then_some(code)
}
