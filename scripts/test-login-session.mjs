import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, existsSync, unlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';

// Runs the actual CLI against synthetic state. Sandbox mode disables ZCode
// process control and upstream probes; no user account data is used.
const exe = resolve(process.argv[2] ?? 'src-tauri/target/debug/zcode-switch.exe');
const root = mkdtempSync(join(tmpdir(), 'zsw-login-test-'));
const failures = [];
let passed = 0;
function run(name, fn) {
  try { fn(); passed++; console.log(`PASS ${name}`); }
  catch (error) { failures.push(name); console.error(`FAIL ${name}: ${error.message}`); }
}
function fixture(name) {
  const home = join(root, name);
  const live = join(home, '.zcode', 'v2');
  const accounts = join(home, '.zcode-switch', 'accounts');
  const arms = join(home, 'arms-store-sandbox', 'ZGVmYXVsdA.json');
  mkdirSync(live, { recursive: true });
  mkdirSync(accounts, { recursive: true });
  mkdirSync(join(home, 'arms-store-sandbox'), { recursive: true });
  const telemetry = join(live, 'telemetry-state.json');
  writeFileSync(telemetry, '{ "deviceMid": "native-install-mid", "lastDailyActiveDate": "2026-10-01" }\n');
  writeFileSync(arms, '{ "_arms_uid": "native-rum-uid", "_arms_session": {"id":"native-session"}, "other": true }\n');
  writeFileSync(join(live, 'provider_config.json'), '{}');
  writeFileSync(join(live, 'setting.json'), '{"providerFamilyDomain":"zai","other":true}');
  const native = {
    zcodejwttoken: 'synthetic-login-token',
    'oauth:active_provider': 'zai',
    'oauth:zai:access_token': 'synthetic-access-token',
    'oauth:zai:user_info': JSON.stringify({ id: 'account-a', username: 'A', displayName: 'A' }),
    'opaque:future-session-field': 'preserve-this-field',
  };
  const credentials = join(live, 'credentials.json');
  writeFileSync(credentials, JSON.stringify(native));
  const invoke = (...args) => {
    const result = spawnSync(exe, ['--cli', '--lang', 'en', ...args], {
      env: { ...process.env, ZCODE_SWITCH_HOME: home, ZCODE_SWITCH_DATA_ROOT: home },
      encoding: 'utf8', timeout: 60000, windowsHide: true,
    });
    assert.ifError(result.error);
    const output = JSON.parse(result.stdout.trim());
    return { ...output, exitCode: result.status };
  };
  const captured = invoke('capture', '--name', 'A');
  assert.equal(captured.ok, true);
  const accountPath = join(accounts, `${captured.id}.json`);
  // Legacy claim context must still load, and must never overwrite native IDs.
  const account = JSON.parse(readFileSync(accountPath));
  assert.equal(account.virtual_device_mid, 'native-install-mid', 'capture did not retain its actual native login context');
  assert.equal(account.virtual_arms_uid, null, 'capture generated an unnecessary RUM identity');
  account.virtual_device_mid = 'legacy-account-mid';
  account.virtual_arms_uid = 'legacy-account-rum-uid';
  writeFileSync(accountPath, JSON.stringify(account));
  const before = [readFileSync(telemetry), readFileSync(arms)];
  return { home, live, arms, telemetry, credentials, native, invoke, id: captured.id, accountPath, before };
}
function assertNativeState(f) {
  assert.deepEqual(readFileSync(f.telemetry), f.before[0], 'native telemetry changed');
  assert.deepEqual(readFileSync(f.arms), f.before[1], 'native RUM session changed');
}
run('forced restore preserves native device, RUM session and opaque credentials', () => {
  const f = fixture('forced');
  writeFileSync(f.credentials, '{}');
  const result = f.invoke('switch', '--id', f.id, '--force', '--no-restart', '--no-hot');
  assert.equal(result.ok, true);
  assert.equal(result.switched, true);
  assert.deepEqual(JSON.parse(readFileSync(f.credentials)), f.native);
  assertNativeState(f);
});
run('already active account preserves native state and refreshed tokens', () => {
  const f = fixture('already');
  const fresh = { ...f.native, zcodejwttoken: 'synthetic-refreshed-token' };
  writeFileSync(f.credentials, JSON.stringify(fresh));
  const result = f.invoke('switch', '--id', f.id, '--force', '--no-restart');
  assert.equal(result.already_active, true);
  assert.deepEqual(JSON.parse(readFileSync(f.credentials)), fresh);
  const saved = JSON.parse(readFileSync(f.accountPath));
  assert.deepEqual(saved.credentials, fresh);
  assert.equal(saved.virtual_device_mid, 'legacy-account-mid');
  assert.equal(saved.virtual_arms_uid, 'legacy-account-rum-uid');
  assertNativeState(f);
});
run('missing native telemetry is left for ZCode to initialize', () => {
  const f = fixture('missing');
  unlinkSync(f.telemetry);
  writeFileSync(f.credentials, '{}');
  assert.equal(f.invoke('switch', '--id', f.id, '--force', '--no-restart').ok, true);
  assert.equal(existsSync(f.telemetry), false, 'switch created a replacement device identity');
  assert.deepEqual(readFileSync(f.arms), f.before[1]);
});
run('expired snapshot is rejected before changing live login or telemetry', () => {
  const f = fixture('expired');
  const account = JSON.parse(readFileSync(f.accountPath));
  account.credentials.zcodejwttoken = `e30.${Buffer.from(JSON.stringify({ exp: 1 })).toString('base64url')}.test`;
  writeFileSync(f.accountPath, JSON.stringify(account));
  writeFileSync(f.credentials, '{}');
  const before = readFileSync(f.credentials);
  const result = f.invoke('switch', '--id', f.id, '--force', '--no-restart');
  assert.equal(result.ok, false);
  assert.equal(result.exitCode, 1);
  assert.match(result.error, /expired/i);
  assert.deepEqual(readFileSync(f.credentials), before);
  assertNativeState(f);
});
run('switch preserves an unsaved native account before restoring the target', () => {
  const f = fixture('preserve');
  const other = { ...f.native, 'oauth:zai:user_info': JSON.stringify({ id: 'account-b' }),
    zcodejwttoken: 'synthetic-other-account-token' };
  writeFileSync(f.credentials, JSON.stringify(other));
  const result = f.invoke('switch', '--id', f.id, '--force', '--no-restart');
  assert.equal(result.ok, true);
  assert.equal(typeof result.preserved_as, 'string');
  const { accounts } = f.invoke('list');
  assert.equal(accounts.length, 2);
  const saved = accounts.find(account => account.id !== f.id);
  assert.deepEqual(JSON.parse(readFileSync(join(f.home, '.zcode-switch', 'accounts', `${saved.id}.json`))).credentials, other);
  assert.deepEqual(JSON.parse(readFileSync(f.credentials)), f.native);
  assertNativeState(f);
});
run('token archival retains the saved claim context and native identity', () => {
  const f = fixture('update');
  const fresh = { ...f.native, zcodejwttoken: 'synthetic-new-native-login' };
  writeFileSync(f.credentials, JSON.stringify(fresh));
  assert.equal(f.invoke('update', '--id', f.id).ok, true);
  const saved = JSON.parse(readFileSync(f.accountPath));
  assert.deepEqual(saved.credentials, fresh);
  assert.equal(saved.virtual_device_mid, 'legacy-account-mid');
  assertNativeState(f);
});
console.log(`${passed}/6 passed; synthetic fixtures: ${root}`);
if (failures.length) process.exitCode = 1;
