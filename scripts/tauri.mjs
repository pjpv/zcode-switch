import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, dirname, join } from "node:path";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const args = process.argv.slice(2);
const env = { ...process.env };
const cargoBin = join(env.CARGO_HOME || join(homedir(), ".cargo"), "bin");
if (existsSync(join(cargoBin, process.platform === "win32" ? "cargo.exe" : "cargo"))) {
  env.PATH = [env.PATH, cargoBin].filter(Boolean).join(delimiter);
}
// Tauri uses CI=true to skip Finder automation when creating a DMG.
if (process.platform === "darwin" && ["build", "bundle"].includes(args[0])) {
  env.CI = "true";
}
const cli = join(dirname(require.resolve("@tauri-apps/cli/package.json")), "tauri.js");
const result = spawnSync(process.execPath, [cli, ...args], { env, stdio: "inherit" });
if (result.error) console.error(result.error.message);
process.exit(result.status ?? 1);
