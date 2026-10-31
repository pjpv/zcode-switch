import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

// Run the shipped renderer with a fake SDK and bridge. No browser, network,
// account state, or real timers are involved in these lifecycle checks.
const source = readFileSync(new URL('../src/captcha.js', import.meta.url), 'utf8')
  .replace(/^import .*;\r?\n/gm, '');

async function fixture({ auto = false, instance = true } = {}) {
  const calls = [];
  const events = [];
  const timers = new Map();
  const elements = new Map();
  let clock = 0;
  let nextTimer = 0;
  let callbacks;
  const flush = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
  const element = id => {
    if (!elements.has(id)) elements.set(id, { hidden: true, addEventListener() {}, focus() {} });
    return elements.get(id);
  };
  const context = vm.createContext({
    URLSearchParams,
    location: { search: `?claimId=claim-under-test&auto=${auto ? 1 : 0}` },
    document: {
      getElementById: element,
      querySelector: element,
      addEventListener() {},
    },
    window: {
      initAliyunCaptcha(options) {
        callbacks = options;
        if (instance) options.getInstance({ startTracelessVerification() {} });
      },
    },
    init() {},
    t: key => key,
    lang: () => 'en',
    stripErr: error => String(error),
    setTimeout(fn, delay) {
      const id = ++nextTimer;
      timers.set(id, { fn, at: clock + delay });
      return id;
    },
    clearTimeout: id => timers.delete(id),
    invoke: async (command, args) => {
      calls.push({ command, args });
      if (command === 'get_state') return { language: 'en' };
      if (command === 'claim_captcha_config') return { enabled: true, scene_id: 'synthetic-scene', region: 'cn' };
      if (command === 'claim_captcha_submit') return;
      throw new Error(`Unexpected invoke: ${command}`);
    },
    emit: async (name, value) => { events.push({ name, value }); },
  });
  vm.runInContext(source, context, { filename: 'src/captcha.js' });
  await flush();
  assert.ok(callbacks, 'renderer must initialize the fake SDK');
  return {
    callbacks, events, flush,
    submits: () => calls.filter(call => call.command === 'claim_captcha_submit'),
    async advance(ms) {
      clock += ms;
      for (const [id, timer] of [...timers]) {
        if (timer.at <= clock && timers.has(id)) { timers.delete(id); timer.fn(); }
      }
      await flush();
    },
  };
}

test('automatic interactive result terminates this attempt before a late SDK success', async () => {
  const f = await fixture({ auto: true });
  f.callbacks.fail('requires interaction');
  f.callbacks.success('late-synthetic-token');
  await f.flush();
  assert.equal(f.events.filter(event => event.name === 'captcha://interactive').length, 1);
  assert.equal(f.events.find(event => event.name === 'captcha://interactive').value.claimId, 'claim-under-test');
  assert.equal(f.submits().length, 0);
});

test('automatic traceless timeout also rejects a subsequent success callback', async () => {
  const f = await fixture({ auto: true });
  await f.advance(8000);
  f.callbacks.success({ captchaVerifyParam: 'late-synthetic-token' });
  await f.flush();
  assert.ok(f.events.some(event => event.name === 'captcha://interactive'));
  assert.equal(f.submits().length, 0);
});

test('manual interactive challenge can complete successfully with the same request ID', async () => {
  const f = await fixture();
  f.callbacks.fail('requires interaction');
  f.callbacks.success({ captchaVerifyParam: 'manual-synthetic-token' });
  await f.flush();
  assert.equal(f.submits().length, 1);
  assert.equal(f.submits()[0].args.param, 'manual-synthetic-token');
  assert.equal(f.submits()[0].args.claimId, 'claim-under-test');
});

test('SDK setup failure rejects late success for either automatic or manual claims', async () => {
  for (const auto of [false, true]) {
    const f = await fixture({ auto });
    f.callbacks.onError();
    f.callbacks.success('late-synthetic-token');
    await f.flush();
    assert.ok(f.events.some(event => event.name === 'captcha://failed'));
    assert.equal(f.submits().length, 0);
  }
});

test('SDK initialization timeout rejects late getInstance and success', async () => {
  const f = await fixture({ instance: false });
  await f.advance(12000);
  let started = false;
  f.callbacks.getInstance({ startTracelessVerification() { started = true; } });
  f.callbacks.success('late-synthetic-token');
  await f.flush();
  assert.equal(started, false);
  assert.equal(f.submits().length, 0);
});

test('repeated success callbacks submit exactly once in both modes', async () => {
  for (const auto of [false, true]) {
    const f = await fixture({ auto });
    f.callbacks.success('first-synthetic-token');
    f.callbacks.success({ captchaVerifyParam: 'second-synthetic-token' });
    await f.flush();
    assert.equal(f.submits().length, 1);
    assert.equal(f.submits()[0].args.param, 'first-synthetic-token');
  }
});
