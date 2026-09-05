import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { esc, toast, openPwModal, installDelegation, dismissSplash } from "./ui.js";
import { ic } from "./icons.js";
import { init, t, lang, stripErr } from "./i18n.js";

const $app = document.getElementById("app");
const isMac = /Macintosh|MacIntel/.test(navigator.userAgent);
if (isMac) document.body.classList.add("mac");
let state = null;
let autostart = false;
let busy = false;

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
    await guard(async () => {
      await invoke("set_zcode_path", { path: input.value.trim() });
      toast(t("s.pathUpdated"));
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
    $app.innerHTML = `<div class="loading">${t("q.loading")}</div>`;
    return;
  }
  const s = state;
  document.title = `Z·SWITCH ${t("s.title")}`;
  $app.innerHTML = `
    <div class="tb-drag" data-tauri-drag-region></div>
    <section class="settings open">
      <div class="set-group">${langSeg(s.language || "zh")}</div>
      <div class="set-label">${t("s.behaviorLabel")}</div>
      <div class="set-group">
        ${toggle(autostart, "actions.toggleAutostart()", t("s.autostart"), t("s.autostartDesc"))}
        ${toggle(s.launch_after_switch, "actions.toggleBehavior('launch')", t("s.launchAfter"), t("s.launchAfterDesc"))}
        ${toggle(s.close_to_tray, "actions.toggleBehavior('tray')", t("s.closeTray"), t("s.closeTrayDesc"))}
        ${toggle(s.hot_switch, "actions.toggleBehavior('hot')", t("s.hotSwitch"), t("s.hotSwitchDesc"))}
      </div>
      <div class="set-label">${t("s.authLabel")}</div>
      <div class="set-group">
        ${toggle(s.auth_proxy_on, "actions.toggleAuthProxy()", t("s.proxyToggle"), t("s.proxyToggleDesc"))}
        <div class="path-line">
          <input class="zcode-path auth-proxy" type="text" value="${esc(s.auth_proxy_url || "")}"
            placeholder="${t("s.proxyPh")}" keydown="onProxyKey(event)">
          <button class="btn-ghost" click="actions.saveProxy()">${t("common.save")}</button>
        </div>
      </div>
      <div class="set-label">${t("s.libLabel")}</div>
      <div class="set-group">
        <div class="lib-row">
          <button class="btn-ghost has-ic" click="actions.importFiles()">${t("s.importBtn")}</button>
          <button class="btn-ghost has-ic" click="actions.exportAll()" ${s.accounts.length ? "" : "disabled"}>${t("s.exportAllBtn")}</button>
        </div>
      </div>
      <div class="set-label">${t("s.pathLabel")}</div>
      <div class="set-group">
        <div class="path-line">
          <input class="zcode-path" type="text" value="${esc(s.zcode_path)}" placeholder="C:\\Program Files\\ZCode\\ZCode.exe" keydown="onPathKey(event)">
          <button class="btn-ghost" click="actions.browsePath()">${t("s.browse")}</button>
          <button class="btn-ghost" click="actions.savePath()">${t("common.save")}</button>
        </div>
      </div>
      <div class="hint">${t("s.hint")}</div>
      <div class="gh-row"><a class="gh-link" href="https://github.com/pjpv/zcode-switch" target="_blank" rel="noopener" click="actions.openGitHub()">${t("s.githubLink")}</a></div>
    </section>`;
}

window.actions = actions;
window.onPathKey = (e) => { if (e.key === "Enter") actions.savePath(); };
window.onProxyKey = (e) => { if (e.key === "Enter") actions.saveProxy(); };
installDelegation();

listen("state-changed", () => {
  refresh().then(render).catch(() => {});
});

(async () => {
  try {
    await refresh();
    render();
    dismissSplash();
  } catch (e) {
    $app.innerHTML = `<div class="loading" style="color:var(--red)">${t("common.loadFail", { e: esc(String(e)) })}</div>`;
    dismissSplash();
  }
})();
