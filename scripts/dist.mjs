import { execSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, statSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const fail = (msg) => {
  console.error(`[dist] ✗ ${msg}`);
  process.exit(1);
};
const mb = (p) => (statSync(p).size / 1024 / 1024).toFixed(2);

const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
const conf = JSON.parse(readFileSync(join(root, "src-tauri/tauri.conf.json"), "utf8"));
const cargoToml = readFileSync(join(root, "src-tauri/Cargo.toml"), "utf8");
const cargoVer = cargoToml.slice(0, cargoToml.indexOf("[lib]")).match(/^\s*version\s*=\s*"([^"]+)"/m)?.[1];
const versions = { "package.json": pkg.version, "tauri.conf.json": conf.version, "Cargo.toml": cargoVer };
const uniq = [...new Set(Object.values(versions))];
if (uniq.length !== 1) {
  fail(`版本号不一致：${Object.entries(versions).map(([k, v]) => `${k}=${v}`).join("  ")}`);
}
const ver = uniq[0];
console.log(`[dist] ✓ 版本一致：v${ver}`);

console.log("[dist] npm run tauri build …");
try {
  execSync("npm run tauri build", { cwd: root, stdio: "inherit" });
} catch {
  fail("构建失败（若是 os error 32：旧实例还在托盘驻留锁住了 exe，退出后重试）");
}

if (process.platform === "darwin") {
  const bundleDir = join(root, "src-tauri/target/release/bundle/dmg");
  if (!existsSync(bundleDir)) fail(`找不到打包目录：${bundleDir}`);
  const packages = readdirSync(bundleDir).filter((f) => f.endsWith(".dmg") && f.includes(`_${ver}_`));
  if (!packages.length) fail(`没有当前版本 ${ver} 的 DMG 安装包`);
  const outDir = join(root, "release");
  mkdirSync(outDir, { recursive: true });
  for (const name of packages) {
    const destination = join(outDir, name);
    copyFileSync(join(bundleDir, name), destination);
    console.log(`[dist] ✓ 安装包  ${name}  (${mb(destination)} MB)`);
  }
  console.log(`[dist] 完成 → ${outDir}`);
  process.exit(0);
}

const bundleDir = join(root, "src-tauri/target/release/bundle/nsis");
if (!existsSync(bundleDir)) fail(`找不到打包目录：${bundleDir}`);
const setups = readdirSync(bundleDir)
  .filter((f) => f.endsWith("-setup.exe"))
  .map((f) => ({ f, t: statSync(join(bundleDir, f)).mtimeMs }))
  .sort((a, b) => b.t - a.t);
if (!setups.length) fail("bundle/nsis 下没有 *-setup.exe");
const setup = setups[0].f;
if (!setup.includes(`_${ver}_`)) {
  fail(`最新安装包 ${setup} 不含当前版本 ${ver} —— 本次构建可能未产出 NSIS 包，请检查上方构建日志`);
}

const rawExe = join(root, "src-tauri/target/release/zcode-switch.exe");
if (!existsSync(rawExe)) fail(`找不到裸 exe：${rawExe}`);

const outDir = join(root, "release");
mkdirSync(outDir, { recursive: true });
const setupDst = join(outDir, setup);
const portableDst = join(outDir, `zcode-switch_${ver}_portable.exe`);
copyFileSync(join(bundleDir, setup), setupDst);
copyFileSync(rawExe, portableDst);

console.log(`[dist] ✓ 安装包  ${setup}  (${mb(setupDst)} MB)`);
console.log(`[dist] ✓ 便携版  ${basename(portableDst)}  (${mb(portableDst)} MB)`);
console.log(`[dist] 完成 → ${outDir}`);
