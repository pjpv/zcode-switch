import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

// The GUI exe owns no console, so a child started without CREATE_NO_WINDOW opens a terminal
// window that takes focus. tauri.conf.json runs this before every build.
const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const sources = join(root, "src-tauri/src");

function rustFiles(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return rustFiles(path);
    return entry.name.endsWith(".rs") ? [path] : [];
  });
}

function isSafeSpawn(lines, index) {
  const nearby = lines.slice(Math.max(0, index - 3), index).join("\n");
  const following = lines.slice(index, index + 12).join("\n");
  return lines[index].includes("detached(")
    || nearby.includes("fn no_window(")
    || nearby.includes("#[cfg(not(windows))]")
    || following.includes(".creation_flags(");
}

test("child processes cannot open a console window", () => {
  const unsafe = rustFiles(sources).flatMap((file) => {
    const lines = readFileSync(file, "utf8").split("\n");
    return lines.flatMap((line, index) =>
      line.includes("Command::new(") && !isSafeSpawn(lines, index)
        ? [`${relative(root, file)}:${index + 1}`]
        : []);
  });
  assert.deepEqual(unsafe, [], "use no_window() or set CREATE_NO_WINDOW");
});
