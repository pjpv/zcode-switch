import { invoke } from "@tauri-apps/api/core";
import { ic } from "./icons.js";
import { t, has, errCode, stripErr } from "./i18n.js";

document.addEventListener("contextmenu", (e) => e.preventDefault());

document.addEventListener("keydown", (e) => {
  if (e.key === "F12") e.preventDefault();
  if (e.ctrlKey && e.shiftKey && ["I", "i", "J", "j", "C", "c"].includes(e.key)) e.preventDefault();
  if (e.ctrlKey && ["u", "s"].includes(e.key.toLowerCase())) e.preventDefault();
});

export function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

export function dismissSplash() {
  const el = document.getElementById("splash");
  if (!el) return;
  el.style.opacity = "0";
  setTimeout(() => el.remove(), 220);
}

export function toast(msg, kind = "ok", detail = "") {
  let zone = document.querySelector(".toast-zone");
  if (!zone) {
    zone = document.createElement("div");
    zone.className = "toast-zone";
    document.body.appendChild(zone);
  }
  const t = document.createElement("div");
  t.className = `toast ${kind}`;
  t.innerHTML = `<span class="t-ic">${ic(kind === "ok" ? "check" : "alert", 16)}</span>
    <span class="t-body">${esc(msg)}${detail ? `<span class="detail">${esc(detail)}</span>` : ""}</span>`;
  zone.appendChild(t);
  setTimeout(() => t.remove(), detail ? 5200 : 3200);
}

function runAttr(expr, event) {
  try {
    const open = expr.indexOf("(");
    if (open < 0 || !expr.trimEnd().endsWith(")")) return;
    let fn = window;
    for (const seg of expr.slice(0, open).trim().split(".")) fn = fn?.[seg];
    if (typeof fn !== "function") return;
    const src = expr.slice(open + 1, expr.trimEnd().length - 1).trim();
    const args = src
      ? src.split(/\s*,\s*/).map((a) => {
          if (a === "event") return event;
          const m = a.match(/^'([^']*)'$/);
          if (m) return m[1];
          return JSON.parse(a);
        })
      : [];
    fn(...args);
  } catch (e) {
    console.warn("attr handler error:", expr, e);
  }
}

export function installDelegation() {
  document.addEventListener("click", (e) => {
    const el = e.target.closest("[click]");
    if (!el) return;
    e.preventDefault();
    runAttr(el.getAttribute("click") || "", e);
  });
  document.addEventListener("keydown", (e) => {
    const el = e.target.closest("[keydown]");
    if (!el) return;
    runAttr(el.getAttribute("keydown") || "", e);
  });
  document.addEventListener("blur", (e) => {
    const el = e.target.closest("[blur]");
    if (!el) return;
    runAttr(el.getAttribute("blur") || "", e);
  }, true);
}

export function openConfirmModal(m) {
  document.querySelector(".cm-mask")?.remove();
  const kind = m.kind || "plain";
  const mask = document.createElement("div");
  mask.className = "cm-mask";
  mask.innerHTML = `
    <div class="cm-panel" role="alertdialog" aria-modal="true" aria-label="${esc(m.title)}">
      <div class="cm-strip ${kind}"></div>
      <div class="cm-head">
        <span class="cm-ic ${kind}">${ic(m.icon || "alert", 21)}</span>
        <div class="cm-title">${esc(m.title)}</div>
      </div>
      ${m.desc ? `<div class="cm-desc">${m.desc}</div>` : ""}
      <div class="cm-actions">
        <button class="btn-ghost cm-no">${esc(m.noLabel || t("common.cancel"))}</button>
        <button class="cm-yes ${kind}">${esc(m.yesLabel || t("common.confirm"))}</button>
      </div>
    </div>`;
  document.body.appendChild(mask);

  const panel = mask.querySelector(".cm-panel");
  const yes = mask.querySelector(".cm-yes");
  const no = mask.querySelector(".cm-no");
  const close = () => {
    document.removeEventListener("keydown", mask._key);
    mask.remove();
  };
  panel.addEventListener("click", (e) => e.stopPropagation());
  mask.addEventListener("click", close);
  no.addEventListener("click", close);
  yes.addEventListener("click", async () => {
    yes.disabled = true; no.disabled = true;
    try { await m.onYes?.(); } finally { close(); }
  });
  mask._key = (e) => {
    if (e.key === "Escape") close();
    if (e.key === "Enter" && e.target === document.body && !yes.disabled) yes.click();
  };
  document.addEventListener("keydown", mask._key);
  (m.focusNo || kind === "danger" ? no : yes).focus();
}

export function openPwModal(m) {
  document.querySelector(".pw-mask")?.remove();
  const isExport = m.mode === "export" || m.mode === "exportAll";
  const title = m.mode === "export" ? t("pw.exportTitle", { name: esc(m.name) })
    : m.mode === "exportAll" ? t("pw.exportAllTitle", { count: m.count })
    : t("pw.importTitle", { count: m.files.length });
  const sub = isExport
    ? t("pw.exportSub", { path: esc(m.path) })
    : t("pw.importSub", { files: m.files.map(([n]) => esc(n)).join(t("common.listSep")) });

  const mask = document.createElement("div");
  mask.className = "pw-mask";
  mask.innerHTML = `
    <div class="pw-panel">
      <div class="pw-title">${title}</div>
      <div class="pw-sub">${sub}</div>
      <input class="pw-input" type="password" id="pw1" placeholder="${isExport ? t("pw.setPw") : t("pw.pw")}" autocomplete="off">
      ${isExport ? `<input class="pw-input" type="password" id="pw2" placeholder="${t("pw.pwAgain")}" autocomplete="off">` : ""}
      <div class="pw-err"></div>
      <div class="pw-actions">
        <button class="btn-ghost pw-cancel">${t("common.cancel")}</button>
        <button class="btn-primary pw-go has-ic">${isExport ? ic("lock", 14) + " " + t("pw.encryptExport") : ic("lockOpen", 14) + " " + t("pw.decryptImport")}</button>
      </div>
    </div>`;
  document.body.appendChild(mask);

  const panel = mask.querySelector(".pw-panel");
  const errEl = mask.querySelector(".pw-err");
  const go = mask.querySelector(".pw-go");
  const input1 = mask.querySelector("#pw1");
  const input2 = mask.querySelector("#pw2");
  panel.addEventListener("click", (e) => e.stopPropagation());
  mask.addEventListener("click", () => mask.remove());
  mask.querySelectorAll(".pw-cancel").forEach((b) => b.addEventListener("click", () => mask.remove()));
  const key = (e) => {
    if (e.key === "Escape") mask.remove();
    if (e.key === "Enter") confirm();
  };
  [input1, input2].forEach((i) => i?.addEventListener("keydown", key));
  input1.focus();

  async function confirm() {
    const pw1 = input1?.value || "";
    const pw2 = input2?.value;
    if (pw1.length < 6) return (errEl.textContent = t("pw.errMinLen"));
    if (pw2 !== undefined && pw2 !== null && pw1 !== pw2) return (errEl.textContent = t("pw.errMismatch"));
    go.disabled = true;
    try {
      if (m.mode === "export") {
        const r = await invoke("export_finalize", { path: m.path, id: m.id, password: pw1 });
        toast(t("pw.exportedToast", { path: r.path }), "ok", t("pw.exportedToastDetail"));
        m.onDone?.(r);
      } else if (m.mode === "exportAll") {
        const r = await invoke("export_all_finalize", { path: m.path, password: pw1 });
        toast(t("pw.exportedAllToast", { count: r.count }), "ok", r.path);
        m.onDone?.(r);
      } else {
        let report = await invoke("import_sealed", { files: m.files, password: pw1 });
        if (m.preErrors?.length) {
          report.errors = [...m.preErrors, ...(report.errors || [])];
        }
        const wrongPw = (report.errors || []).some((e) => errCode(e) === "wrong_password" || String(e).includes("密码错误"));
        if (wrongPw) { errEl.textContent = t("pw.errWrong"); go.disabled = false; return; }
        m.onDone?.(report);
      }
      mask.remove();
    } catch (e) {
      errEl.textContent = stripErr(e);
      go.disabled = false;
    }
  }
  go.addEventListener("click", confirm);
}

export function openProviderModal(m) {
  document.querySelector(".pv-mask")?.remove();
  const mask = document.createElement("div");
  mask.className = "pv-mask";
  const displayOf = (p) => has(`prov.${p.id}`) ? t(`prov.${p.id}`) : p.display;
  mask.innerHTML = `
    <div class="pv-panel">
      <div class="pv-title">${t("prov.title")}</div>
      <div class="pv-sub">${t("prov.sub")}</div>
      <div class="pv-list">
        ${(m.providers || []).map((p) => `
          <button class="pv-item" data-id="${esc(p.id)}">
            <span class="pv-name">${esc(displayOf(p))}</span>
            <span class="pv-arrow">→</span>
          </button>`).join("")}
      </div>
      <div class="pv-actions"><button class="btn-ghost pv-cancel">${t("common.cancel")}</button></div>
    </div>`;
  document.body.appendChild(mask);
  const onKey = (e) => { if (e.key === "Escape") close(); };
  const close = () => { mask.remove(); document.removeEventListener("keydown", onKey); };
  mask.querySelectorAll(".pv-item").forEach((b) => {
    b.addEventListener("click", () => { close(); m.onPick?.(b.dataset.id); });
  });
  mask.querySelector(".pv-cancel").addEventListener("click", close);
  document.addEventListener("keydown", onKey);
  const first = mask.querySelector(".pv-item");
  if (first) first.focus();
}

export function openLoginModal(info, opts = {}) {
  document.querySelector(".login-mask")?.remove();

  const mask = document.createElement("div");
  mask.className = "login-mask";

  const expiresAt = Number(info.expires_at_ms) || Date.now() + 300000;
  const calcRemaining = () => Math.max(0, Math.round((expiresAt - Date.now()) / 1000));

  mask.innerHTML = `
    <div class="login-panel">
      <div class="login-title">${t("oauth.modalTitle")}</div>
      <div class="login-sub">${t("oauth.modalSub")}</div>

      <div class="login-field">
        <label class="login-label">${t("oauth.authUrlLabel")}</label>
        <div class="login-url-row">
          <input class="login-url-input" type="text" readonly value="${esc(info.authorize_url)}" />
          <button class="btn-ghost login-copy-btn has-ic">
            ${ic("copy", 14)} <span class="copy-text">${t("oauth.copyLink")}</span>
          </button>
        </div>
      </div>

      <div class="login-browser-act">
        <button class="btn-primary login-open-btn has-ic">
          ${ic("external", 14)} <span>${t("oauth.openBrowser")}</span>
        </button>
      </div>

      <div class="login-status-box">
        <span class="cap-dot login-dot"></span>
        <div class="login-status-info">
          <div class="login-status-desc">${t("oauth.waiting")}</div>
          <div class="login-countdown">${t("oauth.expiresIn", { sec: calcRemaining() })}</div>
        </div>
      </div>

      <div class="login-manual-section">
        <button type="button" class="login-manual-toggle">${t("oauth.manualToggle")}</button>
        <div class="login-manual-body" style="display:none;">
          <div class="login-manual-hint">${t("oauth.manualHint")}</div>
          <div class="login-manual-row">
            <input class="login-manual-input" type="text" placeholder="${esc(t("oauth.manualPlaceholder"))}" autocomplete="off" />
            <button class="btn-primary login-manual-submit">${t("oauth.manualSubmit")}</button>
          </div>
          <div class="login-manual-err"></div>
        </div>
      </div>

      <div class="login-actions">
        <button class="btn-ghost login-cancel-btn">${t("common.cancel")}</button>
      </div>
    </div>`;

  document.body.appendChild(mask);

  const panel = mask.querySelector(".login-panel");
  const urlInput = mask.querySelector(".login-url-input");
  const copyBtn = mask.querySelector(".login-copy-btn");
  const copyText = mask.querySelector(".copy-text");
  const openBtn = mask.querySelector(".login-open-btn");
  const descEl = mask.querySelector(".login-status-desc");
  const countdownEl = mask.querySelector(".login-countdown");
  const dotEl = mask.querySelector(".login-dot");
  const manualToggle = mask.querySelector(".login-manual-toggle");
  const manualBody = mask.querySelector(".login-manual-body");
  const manualInput = mask.querySelector(".login-manual-input");
  const manualSubmit = mask.querySelector(".login-manual-submit");
  const manualErr = mask.querySelector(".login-manual-err");
  const cancelBtn = mask.querySelector(".login-cancel-btn");

  let copyTimer = null;
  let closed = false;

  const copyUrl = async () => {
    if (calcRemaining() <= 0) return;
    urlInput.select();
    let ok = false;
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(info.authorize_url);
        ok = true;
      }
    } catch {}
    if (!ok) {
      try {
        ok = document.execCommand("copy");
      } catch {}
    }
    if (ok) {
      copyText.textContent = t("oauth.copied");
      clearTimeout(copyTimer);
      copyTimer = setTimeout(() => {
        if (!closed) copyText.textContent = t("oauth.copyLink");
      }, 2000);
    } else {
      toast(t("oauth.copyFail"), "warn");
    }
  };

  copyBtn.addEventListener("click", copyUrl);
  urlInput.addEventListener("click", () => urlInput.select());

  openBtn.addEventListener("click", async () => {
    try {
      await invoke("open_external", { url: info.authorize_url });
    } catch (e) {
      toast(stripErr(e), "err");
    }
  });

  const timer = setInterval(() => {
    if (closed) return;
    const rem = calcRemaining();
    if (rem > 0) {
      countdownEl.textContent = t("oauth.expiresIn", { sec: rem });
    } else {
      clearInterval(timer);
      clearTimeout(copyTimer);
      copyText.textContent = t("oauth.copyLink");
      copyBtn.disabled = true;
      descEl.textContent = t("oauth.expired");
      countdownEl.textContent = "";
      dotEl.className = "cap-dot err";
      openBtn.disabled = true;
    }
  }, 1000);

  manualToggle.addEventListener("click", () => {
    const isHidden = manualBody.style.display === "none";
    manualBody.style.display = isHidden ? "flex" : "none";
    if (isHidden) manualInput.focus();
  });

  const submitManual = async () => {
    const val = manualInput.value.trim();
    if (!val) {
      manualErr.textContent = t("oauth.manualEmpty");
      return;
    }
    manualErr.textContent = "";
    manualSubmit.disabled = true;
    try {
      await invoke("oauth_manual_callback", { flow: info.flow, callbackUrl: val });
    } catch (e) {
      const errStr = stripErr(e);
      manualErr.textContent = errStr === "__superseded__"
        ? t("oauth.expired")
        : errStr === "__attribution__"
        ? t("oauth.manualHint")
        : errStr;
      manualSubmit.disabled = false;
    }
  };

  manualSubmit.addEventListener("click", submitManual);
  manualInput.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      submitManual();
    }
  });

  const close = (cancelled = true) => {
    if (closed) return;
    closed = true;
    clearInterval(timer);
    clearTimeout(copyTimer);
    document.removeEventListener("keydown", onKey);
    mask.remove();
    if (cancelled) {
      invoke("oauth_cancel", { flow: info.flow }).catch(() => {});
      opts.onCancel?.();
    }
  };

  const onKey = (e) => {
    if (e.key === "Escape") close(true);
  };
  document.addEventListener("keydown", onKey);
  panel.addEventListener("click", (e) => e.stopPropagation());
  cancelBtn.addEventListener("click", () => close(true));
  mask.addEventListener("click", (e) => {
    if (e.target === mask) close(true);
  });

  openBtn.focus();

  return {
    flow: info.flow,
    close: (cancelled = false) => close(cancelled),
    setError: (err) => {
      if (err) {
        manualErr.textContent = stripErr(err);
        manualBody.style.display = "flex";
      }
    },
  };
}
