// 开发预览专用：在纯浏览器里模拟 Tauri IPC,便于 UI 开发与截图。
// 仅 dev 使用;`vite build` 的 rollupOptions.input 不含 preview.html,不会进产物。

function pad(n) { return String(n).padStart(2, "0"); }
function fmt(d) {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
const NOW = Date.now();
const at = (min) => fmt(new Date(NOW + min * 60000));

const ACCOUNTS = [
  { id: "acct-1", name: "大号", is_active: true, has_config: true, identity: { username: "zhang_dev", email: "zhang***@gmail.com" } },
  { id: "acct-2", name: "工作号", is_active: false, has_config: true, identity: { username: "li_work" } },
  { id: "acct-3", name: "体验号", is_active: false, has_config: true, identity: { email: "trial***@z.ai" } },
];

const QUOTAS = {
  "acct-1": {
    plan_expire: "2027-07-28",
    plans: [
      {
        tier: "Max", tier_code: "max", name: "GLM Coding Max", expire: "2027-07-28",
        items: [
          { kind: "prompt_count", name: "提示次数（每5小时）", window: "hours:5", percent_used: 3, reset: at(320) },
          { kind: "prompt_count", name: "提示次数（每周）", window: "week", percent_used: 42, reset: at(4200) },
          { kind: "prompt_count", name: "提示次数（每月）", window: "month", percent_used: 12, reset: at(9800) },
        ],
      },
      {
        tier: "体验", tier_code: "trial", name: "ZCode Weekend Build", expire: "2026-08-31",
        items: [
          { name: "GLM-5.3-Flash", total: 300000000, remaining: 90000000, percent_used: 70 },
        ],
      },
    ],
  },
  "acct-2": {
    plan_expire: "2026-12-01",
    plans: [
      {
        tier: "Pro", tier_code: "pro", name: "GLM Coding Pro", expire: "2026-12-01",
        items: [
          { kind: "prompt_count", name: "提示次数（每5小时）", window: "hours:5", percent_used: 74, reset: at(140) },
          { kind: "prompt_count", name: "提示次数（每月）", window: "month", percent_used: 55, reset: at(12000) },
          { name: "GLM-5.3", total: 120000000, remaining: 54000000, percent_used: 55 },
        ],
      },
    ],
  },
  "acct-3": {
    plan_expire: "2026-09-06",
    plans: [
      {
        tier: "体验", tier_code: "trial", name: "GLM Flash 体验", expire: "2026-09-06",
        items: [
          { kind: "prompt_count", name: "提示次数（每5小时）", window: "hours:5", percent_used: 95, reset: at(48) },
          { name: "GLM-5.3-Flash", total: 5000000, remaining: 250000, percent_used: 95 },
        ],
      },
    ],
  },
};

const CLAIMS = {
  "acct-2": [{
    plan_id: "weekend-build",
    name: "ZCode Weekend Build",
    description: "周末限时体验包",
    grant_items: [{ name: "GLM-5.3-Flash", units: 300000000, period: "one_time" }],
    grants: [],
  }],
  "acct-3": [{
    plan_id: "weekend-build",
    name: "ZCode Weekend Build",
    description: "周末限时体验包",
    grant_items: [{ name: "GLM-5.3-Flash", units: 300000000, period: "one_time" }],
    grants: [],
  }],
};

function mockInvoke(cmd, args = {}) {
  switch (cmd) {
    case "get_state":
      return Promise.resolve({
        language: "zh",
        zcode_running: true,
        live_logged_in: true,
        hot_switch: false,
        launch_after_switch: true,
        close_to_tray: true,
        zcode_path_ok: true,
        auth_proxy_on: false,
        auth_proxy_url: "",
        zcode_path: "C:\\Program Files\\ZCode\\ZCode.exe",
        accounts: ACCOUNTS,
      });
    case "get_account_quota":
      return Promise.resolve(QUOTAS[args.id] || { plans: [], items: [] });
    case "claim_preview":
      return Promise.resolve(CLAIMS[args.id] || []);
    case "oauth_providers":
      return Promise.resolve([
        { id: "bigmodel", display: "BigModel" },
        { id: "zai", display: "z.ai" },
      ]);
    case "autostart_status":
      return Promise.resolve(false);
    case "reveal_main":
    case "open_settings":
    case "launch_zcode":
    case "kill_zcode":
    case "switch_to":
      return Promise.resolve({ name: "工作号", hot: false, killed: false, preserved_as: null, launched: false, config_stale: false });
    default:
      return Promise.resolve({});
  }
}

let cbId = 1;
window.__TAURI_INTERNALS__ = {
  invoke: (cmd, args) => mockInvoke(cmd, args),
  transformCallback: (cb) => {
    const id = cbId++;
    window[`_${id}`] = cb;
    return id;
  },
  unregisterCallback: () => {},
  metadata: {
    currentWindow: { label: "main" },
    currentWebview: { label: "main", windowLabel: "main" },
  },
  plugins: {},
};
