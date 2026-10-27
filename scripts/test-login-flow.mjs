#!/usr/bin/env node
import fs from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const source = fs.readFileSync(join(root, 'src/main.js'), 'utf8');
const add = source.slice(source.indexOf('  async addAccount() {'), source.indexOf('  async claim(id) {'));
const listener = source.slice(source.indexOf('function handleOauthDone(p) {'), source.indexOf('listen("state-changed",'));

function createHarness() {
  let picked, eventHandler;
  let opened = 0, closed = 0, refreshed = 0;
  const toasted = [];
  const cancelled = [];

  const sandbox = {
    invoke: async (name, args) => {
      if (name === 'oauth_providers') return [{ id: 'zai' }, { id: 'bigmodel' }];
      if (name === 'oauth_cancel') {
        cancelled.push(args?.flow);
        return {};
      }
      throw new Error(`Unhandled invoke: ${name}`);
    },
    openProviderModal: (options) => { picked = options.onPick; },
    pickProvider: (id) => picked(id),
    emitDone: (p) => eventHandler({ payload: p }),
    openLoginModal: (info) => {
      opened++;
      return {
        flow: info.flow,
        close: (userCancel) => {
          closed++;
          if (userCancel) cancelled.push(info.flow);
        },
      };
    },
    listen: (name, handler) => { eventHandler = handler; },
    toast: (msg, type) => { toasted.push({ msg, type }); },
    t: (x) => x,
    stripErr: String,
    refresh: async () => { refreshed++; },
    uiLocked: () => false,
    render: () => {},
    enrollAccounts: () => {},
  };

  vm.createContext(sandbox);
  vm.runInContext(`
    let activeLoginModal = null;
    let currentLoginFlow = null;
    let inFlightLoginId = 0;
    let earlyDoneEvents = {};
    const actions = {${add}};
    ${listener};
    globalThis.runAdd = () => actions.addAccount();
    globalThis.modalAlive = () => !!activeLoginModal;
  `, sandbox);

  return { sandbox, cancelled, toasted, getOpened: () => opened, getClosed: () => closed, getRefreshed: () => refreshed };
}

// Test 1: Early terminal event (poll fail arrives before oauth_begin resolves)
async function testEarlyTerminal() {
  const h = createHarness();
  h.sandbox.invoke = async (name, args) => {
    if (name === 'oauth_providers') return [{ id: 'zai' }];
    if (name === 'oauth_begin') {
      h.sandbox.emitDone({ ok: false, flow: 'flow-early-fail', error: 'terminal poll failure' });
      return { flow: 'flow-early-fail', authorize_url: 'https://example.test', expires_at_ms: Date.now() + 300000 };
    }
    if (name === 'oauth_cancel') return {};
    throw new Error(name);
  };
  await h.sandbox.runAdd();
  await h.sandbox.pickProvider('zai');

  assert.equal(h.sandbox.modalAlive(), false, 'modal should not be open');
  assert.equal(h.getOpened(), 0, 'modal should never open on early terminal failure');
  assert.equal(h.toasted.length, 1, 'error should be toasted');
  assert.equal(h.toasted[0].type, 'err');
  console.log('✓ Test 1 Passed: Early terminal event handled without zombie modal');
}

// Test 2: Normal flow (modal opened, then done event arrives)
async function testNormalSuccess() {
  const h = createHarness();
  h.sandbox.invoke = async (name, args) => {
    if (name === 'oauth_providers') return [{ id: 'zai' }];
    if (name === 'oauth_begin') {
      return { flow: 'flow-norm', authorize_url: 'https://example.test', expires_at_ms: Date.now() + 300000 };
    }
    if (name === 'oauth_cancel') return {};
    throw new Error(name);
  };
  await h.sandbox.runAdd();
  await h.sandbox.pickProvider('zai');

  assert.equal(h.sandbox.modalAlive(), true, 'modal should be open');
  assert.equal(h.getOpened(), 1, 'modal opened once');

  h.sandbox.emitDone({ ok: true, flow: 'flow-norm', name: 'acc-1' });

  assert.equal(h.sandbox.modalAlive(), false, 'modal should close on success');
  assert.equal(h.getClosed(), 1, 'modal closed once');
  assert.equal(h.getRefreshed(), 1, 'accounts refreshed');
  console.log('✓ Test 2 Passed: Normal flow opens modal and closes on success');
}

// Test 3: Rapid sequential provider picks (A then B before A resolves)
async function testRapidSequential() {
  const h = createHarness();
  let resolveA;
  const pA = new Promise((r) => { resolveA = r; });

  h.sandbox.invoke = async (name, args) => {
    if (name === 'oauth_providers') return [{ id: 'zai' }, { id: 'bigmodel' }];
    if (name === 'oauth_begin') {
      if (args.provider === 'zai') return pA;
      return { flow: 'flow-B', authorize_url: 'https://example.test', expires_at_ms: Date.now() + 300000 };
    }
    if (name === 'oauth_cancel') {
      h.cancelled.push(args?.flow);
      return {};
    }
    throw new Error(name);
  };

  await h.sandbox.runAdd();
  const pickA = h.sandbox.pickProvider('zai');
  const pickB = h.sandbox.pickProvider('bigmodel');
  await pickB;

  assert.equal(h.getOpened(), 1, 'modal opened for B');

  // Now resolve A late
  resolveA({ flow: 'flow-A', authorize_url: 'https://example.test', expires_at_ms: Date.now() + 300000 });
  await pickA;

  assert.equal(h.getOpened(), 1, 'late A should not open another modal');
  assert.ok(h.cancelled.includes('flow-A'), 'flow-A should be cancelled');
  console.log('✓ Test 3 Passed: Out-of-order provider pick resolutions handled safely');
}

// Test 4: Stale event for an old flow does not affect current modal
async function testStaleEventIgnored() {
  const h = createHarness();
  h.sandbox.invoke = async (name) => {
    if (name === 'oauth_providers') return [{ id: 'zai' }];
    if (name === 'oauth_begin') {
      return { flow: 'flow-current', authorize_url: 'https://example.test', expires_at_ms: Date.now() + 300000 };
    }
    if (name === 'oauth_cancel') return {};
    throw new Error(name);
  };
  await h.sandbox.runAdd();
  await h.sandbox.pickProvider('zai');

  assert.equal(h.sandbox.modalAlive(), true, 'current modal is open');

  // Stale event for another flow arrives
  h.sandbox.emitDone({ ok: false, flow: 'flow-old-stale', error: 'stale fail' });

  assert.equal(h.sandbox.modalAlive(), true, 'current modal should still be open');
  assert.equal(h.getClosed(), 0, 'modal not closed by stale event');
  assert.equal(h.toasted.length, 0, 'stale event should not toast');
  console.log('✓ Test 4 Passed: Stale event ignored');
}

await testEarlyTerminal();
await testNormalSuccess();
await testRapidSequential();
await testStaleEventIgnored();
console.log('\nAll 4 frontend flow lifecycle tests PASSED!');
