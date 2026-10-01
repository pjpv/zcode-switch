
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { init, t, lang, stripErr } from "./i18n.js";

const SDK_URL = "https://o.alicdn.com/captcha-frontend/aliyunCaptcha/AliyunCaptcha.js";
const TRACELESS_TIMEOUT = 8000;
const SETUP_TIMEOUT = 12000;
const query = new URLSearchParams(location.search);
const claimId = query.get("claimId");
const autoClaim = query.get("auto") === "1";

const $text = document.getElementById("cap-text");
const $detail = document.getElementById("cap-detail");
const $dot = document.getElementById("cap-dot");
const $btn = document.getElementById("cap-btn");

function status(text, tone = "run") {
  $text.textContent = text;
  $dot.className = "cap-dot" + (tone === "ok" ? " ok" : tone === "err" ? " err" : "");
}

function detail(text) {
  $detail.textContent = text || "";
}

document.addEventListener("securitypolicyviolation", (e) => {
  detail(t("c.cspBlocked", { directive: e.violatedDirective, uri: String(e.blockedURI).slice(0, 70) }));
});

const notifyStuck = () => { emit("captcha://interactive", { claimId }).catch(() => {}); };

function setupFailed(text, message = "") {
  clearTimeout(tracelessTimer);
  clearTimeout(initTimer);
  setupStopped = true;
  status(text, "err");
  detail(message);
  $btn.hidden = true;
  emit("captcha://failed", {
    claimId,
    message: message ? `${text}: ${message}` : text,
  }).catch(() => {});
}

function loadSdk() {
  return new Promise((resolve, reject) => {
    if (typeof window.initAliyunCaptcha === "function") return resolve();
    const s = document.createElement("script");
    s.src = SDK_URL;
    const timer = setTimeout(() => {
      s.remove();
      reject(new Error(t("c.sdkFail")));
    }, SETUP_TIMEOUT);
    s.onload = () => {
      clearTimeout(timer);
      if (typeof window.initAliyunCaptcha === "function") resolve();
      else reject(new Error(t("c.sdkFail")));
    };
    s.onerror = () => {
      clearTimeout(timer);
      s.remove();
      reject(new Error(t("c.sdkFail")));
    };
    document.head.appendChild(s);
  });
}

let submitted = false;
let region = null;
let tracelessTimer = 0;
let initTimer = 0;
let setupStopped = false;

async function run() {
  try {
    const st = await invoke("get_state");
    if (st?.language) init(st.language);
  } catch { }
  document.title = t("c.title");
  $btn.textContent = t("c.btn");
  document.querySelector(".cap-foot").textContent = t("c.foot");
  status(t("c.preparing"));

  let cfg;
  try {
    cfg = await invoke("claim_captcha_config");
  } catch (e) {
    setupFailed(t("c.cfgFail"), stripErr(e));
    return;
  }
  if (!cfg.enabled || !cfg.scene_id) {
    setupFailed(t("c.cfgUnavailable"), t("c.cfgUnavailableDetail"));
    return;
  }
  region = cfg.region || null;
  // The SDK reads this global while its script initializes.
  window.AliyunCaptchaConfig = { region: cfg.region, prefix: cfg.prefix };
  try {
    await loadSdk();
  } catch (e) {
    setupFailed(e.message || t("c.sdkFail"));
    return;
  }

  status(t("c.traceless"));

  const submit = (param) => {
    if (submitted || setupStopped || typeof param !== "string" || !param.trim()) return;
    submitted = true;
    clearTimeout(tracelessTimer);
    clearTimeout(initTimer);
    status(t("c.passed"));
    invoke("claim_captcha_submit", { param, region, claimId }).catch((e) => {
      status(t("c.claimReqFail"), "err");
      detail(stripErr(e));
      emit("captcha://failed", {
        claimId,
        message: stripErr(e),
        terminal: true,
      }).catch(() => {});
    });
  };

  const interactive = (why) => {
    if (submitted || setupStopped) return;
    // Auto-claim must stop synchronously before notifying the main window.
    // Otherwise an SDK success callback can race the asynchronous cancellation.
    if (autoClaim) setupStopped = true;
    notifyStuck();
    clearTimeout(tracelessTimer);
    status(t("c.interactive"));
    $btn.hidden = false;
    $btn.focus();
    if (why) detail(typeof why === "string" ? why.slice(0, 120) : JSON.stringify(why).slice(0, 120));
  };

  try {
    initTimer = setTimeout(() => setupFailed(t("c.initFail")), SETUP_TIMEOUT);
    window.initAliyunCaptcha({
      SceneId: cfg.scene_id,
      mode: "popup",
      language: lang() === "en" ? "en" : "zh-CN",
      showErrorTip: false,
      element: "#cap-holder",
      button: "#cap-btn",
      getInstance: (instance) => {
        if (setupStopped) return;
        clearTimeout(initTimer);
        if (typeof instance.startTracelessVerification === "function") {
          tracelessTimer = setTimeout(interactive, TRACELESS_TIMEOUT);
          instance.startTracelessVerification();
        } else {
          interactive();
        }
      },
      success: (param) => submit(typeof param === "string" ? param : param?.captchaVerifyParam),
      fail: (p) => interactive(p),
      onError: () => { if (!submitted) setupFailed(t("c.initFail")); },
    });
  } catch (e) {
    setupFailed(t("c.initFail"), String(e));
  }
}

$btn.addEventListener("click", () => {
  if (!$btn.hidden) status(t("c.inPopup"));
});

run();
