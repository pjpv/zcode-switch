#!/usr/bin/env node
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

function loadDict(f) {
  const src = readFileSync(join(root, "src/locales", f), "utf8");
  const keys = new Set();
  for (const m of src.matchAll(/^\s*"([^"]+)"\s*:/gm)) keys.add(m[1]);
  return keys;
}

const zh = loadDict("zh.js");
const en = loadDict("en.js");

const errs = [];
for (const k of zh) if (!en.has(k)) errs.push(`en 缺 key: ${k}`);
for (const k of en) if (!zh.has(k)) errs.push(`zh 缺 key: ${k}`);

const DYNAMIC = [
  "q.win.daily", "q.win.weekly", "q.win.monthly", "q.win.cycle",
  "prov.bigmodel", "prov.zai",
  "m.bitProbeNoPlan", "m.bitProbeAuthFailed",
];

const callRe = /\bt\(\s*(["'])((?:(?!\1).)+)\1/g; // 只匹配引号串；模板串走 DYNAMIC
const files = readdirSync(join(root, "src")).filter((f) => f.endsWith(".js"));
for (const f of files) {
  const src = readFileSync(join(root, "src", f), "utf8");
  for (const m of src.matchAll(callRe)) {
    const key = m[2];
    if (!zh.has(key) || !en.has(key)) errs.push(`${f}: 用了未定义 key: ${key}`);
  }
}
for (const k of DYNAMIC) {
  if (!zh.has(k) || !en.has(k)) errs.push(`动态家族缺 key: ${k}`);
}

if (errs.length) {
  console.error("✗ i18n key 检查失败:\n  " + errs.join("\n  "));
  process.exit(1);
}
console.log(`✓ i18n keys OK（zh/en 各 ${zh.size} 个，静态调用点 + 动态家族全覆盖）`);
