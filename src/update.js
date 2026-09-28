import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { esc, openConfirmModal, toast } from "./ui.js";
import { t, stripErr } from "./i18n.js";

let installing = false;
let listening = false;
const listeners = new Set();

export function checkAppUpdate() {
  return invoke("check_app_update");
}

export function formatProgress(p) {
  if (!p || !p.phase) return "";
  if (p.phase === "download") {
    if (p.total > 0) {
      const pct = Math.min(100, Math.floor((p.received * 100) / p.total));
      return t("u.progress", { pct });
    }
    return t("u.downloading");
  }
  if (p.phase === "checksum" || p.phase === "verify") return t("u.verifying");
  if (p.phase === "apply") return t("u.applying");
  return "";
}

export function bindUpdateProgress(applyText) {
  listeners.add(applyText);
  if (!listening) {
    listening = true;
    listen("update-progress", (ev) => {
      const text = formatProgress(ev.payload || {});
      if (!text) return;
      for (const fn of listeners) fn(text);
    });
  }
}

function confirmDesc(info) {
  if (!info.installable) return info.reason === "dev" ? t("u.confirmPage") : t("u.confirmManual");
  if (info.mode === "setup") return t("u.confirmQuit");
  if (info.quitting) return t("u.confirmRestart");
  return t("u.confirmOpen");
}

export function confirmAndInstall(info, { onStart, onEnd } = {}) {
  if (!info?.available || installing) return;
  openConfirmModal({
    title: t("u.confirmTitle", { ver: info.latest }),
    desc: esc(confirmDesc(info)),
    yesLabel: info.installable ? t("u.install") : t("u.openPage"),
    onYes: async () => {
      if (installing) return;
      installing = true;
      onStart?.();
      try {
        const r = await invoke("install_app_update");
        if (r?.quitting) toast(info.mode === "setup" ? t("u.installingQuit") : t("u.restarting"));
        else if (!info.installable || r?.mode === "page") toast(t("u.openedPage"));
        else toast(t("u.opened"));
      } catch (e) {
        toast(stripErr(e), "err");
      } finally {
        installing = false;
        onEnd?.();
      }
    },
  });
}
