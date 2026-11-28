import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { esc, toast, openPwModal, installDelegation, dismissSplash } from "./ui.js";
import { ic } from "./icons.js";
import { init, t, lang, stripErr } from "./i18n.js";

const $app = document.getElementById("app");
let state = null;
let autostart = false;
let busy = false;
let appVer = "";

async function refresh() {
  state = await invoke("get_state");
  autostart = await invoke("autostart_status").catch(() => false);
  if (state?.language) init(state.language);
}

async function guard(fn) {
  if (busy) return;
  busy = true;
  try {
    await fn();
  } catch (e) {
    toast(stripErr(e), "err");
  } finally {
    busy = false;
  }
}

const actions = {
  async refresh() { await refresh(); render(); },

  async openGitHub() {
    await invoke("open_external", { url: "https://github.com/pjpv/zcode-switch" });
  },

  async setLang(l) {
    if (l === lang()) return;
    await guard(async () => {
      await invoke("set_language", { lang: l });
      await refresh(); render();
    });
  },

  async toggleAutostart() {
    await guard(async () => {
      const v = await invoke("autostart_set", { enable: !autostart });
      autostart = v;
      toast(v ? t("s.autostartOnToast") : t("s.autostartOffToast"));
      render();
    });
  },

  async toggleBehavior(key) {
    await guard(async () => {
      await invoke("set_behavior", {
        launchAfterSwitch: key === "launch" ? !state.launch_after_switch : null,
        closeToTray: key === "tray" ? !state.close_to_tray : null,
        hotSwitch: key === "hot" ? !state.hot_switch : null,
      });
      await refresh(); render();
      toast(t("s.savedToast"));
    });
  },

  async exportAll() {
    await guard(async () => {
      const p = await invoke("export_all_pick_path");
      if (!p.picked) { toast(t("m.exportCanceled")); return; }
      openPwModal({ mode: "exportAll", path: p.path, count: p.count, onDone: () => actions.refresh() });
    });
  },

  async importFiles() {
    await guard(async () => {
      const p = await invoke("import_pick_files");
      if (!p.picked) return;
      const sealed = p.sealed || [];
      const preErrors = p.errors || [];
      if (sealed.length) {
        openPwModal({ mode: "import", files: sealed, preErrors, onDone: (rep) => actions.finishImport(rep) });
        return;
      }
      actions.finishImport({ added: [], skipped: [], errors: preErrors });
    });
  },

  finishImport(report) {
    if (report.added.length === 0 && report.skipped.length === 0) {
      toast(t("s.importNone"), "err", report.errors.join(t("common.listSep")) || undefined);
    } else {
      const parts = [];
      if (report.added.length) parts.push(t("s.importAdded", { count: report.added.length, names: report.added.join(t("common.listSep")) }));
      if (report.skipped.length) parts.push(t("s.importSkipped", { count: report.skipped.length }));
      if (report.errors.length) parts.push(t("s.importFailed", { count: report.errors.length }));
      toast(parts[0], report.errors.length ? "err" : "ok", parts.slice(1).join(t("common.listSep")));
    }
    refresh().then(render);
  },

  async browsePath() {
    await guard(async () => {
      const r = await invoke("pick_zcode_path");
      if (r.picked) {
        await invoke("set_zcode_path", { path: r.path });
        toast(t("s.pathUpdated"));
        await refresh(); render();
      }
    });
  },

  async savePath() {
    const input = document.querySelector(".settings input.zcode-path:not(.auth-proxy)");
    if (!input) return;
    const v = input.value.trim();
    await guard(async () => {
      await invoke("set_zcode_path", { path: v });
      toast(v ? t("s.pathUpdated") : t("s.pathAuto"));
      await refresh(); render();
    });
  },

  async toggleAuthProxy() {
    const input = document.querySelector(".settings input.auth-proxy");
    const url = (input?.value || "").trim() || state.auth_proxy_url || null;
    await guard(async () => {
      await invoke("set_auth_proxy", { on: !state.auth_proxy_on, url });
      await refresh(); render();
      toast(state.auth_proxy_on ? t("s.proxyOnToast") : t("s.proxyOffToast"), "ok", t("s.proxyOnDetail"));
    });
  },

  async saveProxy() {
    const input = document.querySelector(".settings input.auth-proxy");
    if (!input) return;
    await guard(async () => {
      await invoke("set_auth_proxy", { on: state.auth_proxy_on, url: input.value.trim() });
      await refresh(); render();
      toast(t("s.proxySaved"), "ok",
        state.auth_proxy_on ? t("s.proxySavedOn") : t("s.proxySavedOff"));
    });
  },

  async toggleGateway() {
    await guard(async () => {
      const r = await invoke("gateway_set", { enabled: !state.gateway_enabled, port: null, key: null, regenerateKey: null, solverPython: null, solverScript: null });
      await refresh(); render();
      toast(r.running ? t("s.gwOnToast", { port: r.port }) : t("s.gwOffToast"), "ok",
        r.running ? (r.key_set ? t("s.gwKeyOk") : t("s.gwKeyMissing")) : undefined);
    });
  },

  async saveGatewayNet() {
    const port = parseInt(document.querySelector(".settings input.gw-port")?.value || "", 10);
    const key = (document.querySelector(".settings input.gw-key")?.value || "").trim();
    await guard(async () => {
      const r = await invoke("gateway_set", {
        enabled: null, port: Number.isFinite(port) && port > 0 ? port : null,
        key: key || null, regenerateKey: null, solverPython: null, solverScript: null,
      });
      await refresh(); render();
      toast(t("s.gwSaved", { port: r.port }), "ok", r.running ? t("s.gwSavedRunning") : t("s.gwSavedStopped"));
    });
  },

  async genGatewayKey() {
    await guard(async () => {
      const r = await invoke("gateway_set", { enabled: null, port: null, key: null, regenerateKey: true, solverPython: null, solverScript: null });
      await refresh(); render();
      const input = document.querySelector(".settings input.gw-key");
      if (input && r.key) { input.value = r.key; }
      toast(t("s.gwKeyGen"), "ok");
    });
  },

  async saveSolver() {
    const py = (document.querySelector(".settings input.gw-solver-py")?.value || "").trim();
    const sc = (document.querySelector(".settings input.gw-solver-sc")?.value || "").trim();
    await guard(async () => {
      await invoke("gateway_set", { enabled: null, port: null, key: null, regenerateKey: null, solverPython: py, solverScript: sc });
      await refresh(); render();
      toast(t("s.gwSolverSaved"));
    });
  },
};

const toggle = (on, onclickAttr, label, desc) => `
  <div class="tog-row">
    <div class="tog-info"><div class="tog-label">${label}</div><div class="tog-desc">${desc}</div></div>
    <button class="toggle${on ? " on" : ""}" role="switch" aria-checked="${on}" aria-label="${label}" click="${onclickAttr}">
      <span class="knob"></span>
    </button>
  </div>`;

const langSeg = (cur) => `
  <div class="tog-row">
    <div class="tog-info"><div class="tog-label">${t("s.langLabel")}</div></div>
    <div class="lang-seg" role="radiogroup" aria-label="${t("s.langLabel")}">
      <button class="lang-opt${cur === "zh" ? " on" : ""}" role="radio" aria-checked="${cur === "zh"}" click="actions.setLang('zh')">${t("s.langZh")}</button>
      <button class="lang-opt${cur === "en" ? " on" : ""}" role="radio" aria-checked="${cur === "en"}" click="actions.setLang('en')">${t("s.langEn")}</button>
    </div>
  </div>`;

function render() {
  if (!state) {
    $app.innerHTML = `<div class="loading">LOADING</div>`;
    return;
  }
  const s = state;
  document.title = `Z·SWITCH ${t("s.title")}`;
  $app.innerHTML = `
    <header class="topbar">
      <div class="wordmark">Z·SWITCH <span class="ver">/ ${t("s.title")}</span></div>
    </header>
    <section class="settings open">
      ${langSeg(s.language || "zh")}
      <label>BEHAVIOR · ${t("s.behaviorLabel")}</label>
      ${toggle(autostart, "actions.toggleAutostart()", t("s.autostart"), t("s.autostartDesc"))}
      ${toggle(s.launch_after_switch, "actions.toggleBehavior('launch')", t("s.launchAfter"), t("s.launchAfterDesc"))}
      ${toggle(s.close_to_tray, "actions.toggleBehavior('tray')", t("s.closeTray"), t("s.closeTrayDesc"))}
      ${toggle(s.hot_switch, "actions.toggleBehavior('hot')", t("s.hotSwitch"), t("s.hotSwitchDesc"))}
      <label style="margin-top:14px">${t("s.authLabel")}</label>
      ${toggle(s.auth_proxy_on, "actions.toggleAuthProxy()", t("s.proxyToggle"), t("s.proxyToggleDesc"))}
      <div class="path-line" style="margin-top:6px">
        <input class="zcode-path auth-proxy" type="text" value="${esc(s.auth_proxy_url || "")}"
          placeholder="${t("s.proxyPh")}" keydown="onProxyKey(event)">
        <button class="btn-ghost" click="actions.saveProxy()">${t("common.save")}</button>
      </div>
      <label style="margin-top:14px">${t("s.gwLabel")}</label>
      ${toggle(s.gateway_enabled, "actions.toggleGateway()", t("s.gwToggle"), t("s.gwToggleDesc"))}
      <div class="path-line" style="margin-top:6px">
        <input class="zcode-path gw-port" type="text" inputmode="numeric" value="${esc(String(s.gateway_port || 8787))}">
        <input class="zcode-path gw-key" type="text" value="" placeholder="${s.gateway_key_set ? t("s.gwKeySetPh") : t("s.gwKeyEmptyPh")}" keydown="onGatewayKey(event)">
        <button class="btn-ghost" click="actions.genGatewayKey()">${t("s.gwGenKey")}</button>
        <button class="btn-ghost" click="actions.saveGatewayNet()">${t("common.save")}</button>
      </div>
      <div class="hint">${t("s.gwHint")}</div>
      <div class="path-line" style="margin-top:4px">
        <input class="zcode-path gw-solver-py" type="text" value="${esc(s.captcha_solver_python || "")}" placeholder="${t("s.gwSolverPyPh")}">
        <input class="zcode-path gw-solver-sc" type="text" value="${esc(s.captcha_solver_script || "")}" placeholder="${t("s.gwSolverScPh")}">
        <button class="btn-ghost" click="actions.saveSolver()">${t("common.save")}</button>
      </div>
      <div class="hint">${t("s.gwSolverHint")}</div>
      <label style="margin-top:14px">${t("s.libLabel")}</label>
      <div class="lib-row">
        <button class="btn-ghost has-ic" click="actions.importFiles()">${ic("import", 14)} ${t("s.importBtn")}</button>
        <button class="btn-ghost has-ic" click="actions.exportAll()" ${s.accounts.length ? "" : "disabled"}>${ic("exportAll", 14)} ${t("s.exportAllBtn")}</button>
      </div>
      <label style="margin-top:14px">${t("s.pathLabel")}</label>
      <div class="path-line">
        <input class="zcode-path" type="text" value="${esc(s.zcode_path)}" placeholder="C:\\Program Files\\ZCode\\ZCode.exe" keydown="onPathKey(event)">
        <button class="btn-ghost" click="actions.browsePath()">${t("s.browse")}</button>
        <button class="btn-ghost" click="actions.savePath()">${t("common.save")}</button>
      </div>
      <div class="hint">${t("s.hint")}</div>
      <div class="gh-row"><button class="gh-link" click="actions.openGitHub()">${t("s.githubLink")}</button>${appVer ? `<span class="ver">v${esc(appVer)}</span>` : ""}</div>
    </section>`;
}

window.actions = actions;
window.onPathKey = (e) => { if (e.key === "Enter") actions.savePath(); };
window.onProxyKey = (e) => { if (e.key === "Enter") actions.saveProxy(); };
window.onGatewayKey = (e) => { if (e.key === "Enter") actions.saveGatewayNet(); };
installDelegation();

listen("state-changed", () => {
  refresh().then(render).catch(() => {});
});

(async () => {
  try {
    appVer = await invoke("app_version").catch(() => "");
    await refresh();
    render();
    dismissSplash();
  } catch (e) {
    $app.innerHTML = `<div class="loading" style="color:var(--red)">${t("common.loadFail", { e: esc(String(e)) })}</div>`;
    dismissSplash();
  }
})();
