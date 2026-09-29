import { readFileSync } from "node:fs";
import vm from "node:vm";
import { randomUUID } from "node:crypto";
import { createRequestQueue } from "../src/request-queue.js";

export const turn = () => new Promise(setImmediate);

export function harness(file, invoke, extra = {}) {
  const listeners = new Map(), timers = new Map(), nodes = new Map(), toasts = [];
  let timerId = 0;
  const node = (id) => {
    if (!nodes.has(id)) nodes.set(id, { hidden: true, textContent: "", className: "", focus() {}, remove() {},
      addEventListener(name, fn) { this[name] = fn; } });
    return nodes.get(id);
  };
  const context = vm.createContext({
    console, URLSearchParams, crypto: { randomUUID }, createRequestQueue,
    document: { getElementById: node, querySelector: node, addEventListener() {}, createElement: () => ({ remove() {} }), head: { appendChild() {} } },
    window: {}, location: { search: "?claimId=test-claim", reload() {} },
    invoke, listen: (name, fn) => listeners.set(name, fn), emit: async () => {},
    setTimeout: (fn, ms) => { timers.set(++timerId, { fn, ms }); return timerId; },
    clearTimeout: (id) => timers.delete(id), setInterval: () => ++timerId, clearInterval() {},
    t: (key, data) => key + (data ? JSON.stringify(data) : ""), init() {}, lang: () => "en", localeTag: () => "en-US", has: () => true,
    stripErr: String, esc: (s) => String(s).replaceAll("<", "&lt;"), ic: () => "", toast: (...args) => toasts.push(args),
    installDelegation() {}, dismissSplash() {}, ...extra,
  });
  let source = readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8").replace(/^import .*;\r?\n/gm, "");
  // Exercise the actual handlers; startup polling is independently driven by the tests.
  if (file === "main.js") source = source.slice(0, source.lastIndexOf("(async () => {"));
  vm.runInContext(source, context, { filename: file });
  const run = (code) => vm.runInContext(code, context);
  if (file === "main.js") run("render = () => {}; state = { accounts: [], auto_claim: false };");
  return { context, run, listeners, timers, nodes, toasts };
}
