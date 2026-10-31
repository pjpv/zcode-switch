import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { zh } from '../src/locales/zh.js';

const main = readFileSync(new URL('../src/main.js', import.meta.url), 'utf8');
const code = main.slice(main.indexOf('function quotaBarHtml('), main.indexOf('function captureScroll('));
function render(data) {
  return vm.runInNewContext(`${code}\nacctQuotaSlot('a')`, {
    acctQuota: { a: { data } }, claimable: {},
    lang: () => 'zh', has: () => false,
    t: (key, args = {}) => Object.entries(args).reduce((s, [k, v]) => s.replaceAll(`{${k}}`, v), zh[key] || key),
    esc: s => String(s ?? '').replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('"', '&quot;'),
  });
}
test('one claimed plan with no balances displays both pending grants and dates', () => {
  const html = render({ items: [], plans: [{ name: 'ZCode 1024 Dev Day', tier: 'Start Plan',
    tier_code: 'start', expire: '2026-10-26 09:00', items: [], pending_grants: [
      { name: 'GLM-5.3-Flash', units: 300000000, unit: 'token', effective_at: '2026-10-23 23:00' },
      { name: 'GLM-5.3', units: 200000000, unit: 'token', effective_at: '2026-10-23 23:00' },
    ] }] });
  for (const expected of ['ZCode 1024 Dev Day', '3亿 token', '2亿 token', '待生效 · 2026-10-23 23:00', '2026-10-26']) {
    assert.ok(html.includes(expected), expected);
  }
  assert.ok(!html.includes('当前没有有效额度'));
  assert.ok(!html.includes('qbar'), 'pending grants must not look like spendable balance');
});
test('unclaimed account keeps the empty quota message', () => {
  assert.ok(render({ items: [], plans: [] }).includes(zh['q.noActivePlan']));
});
test('active token balance still renders its remaining amount', () => {
  const items = [
    { name: 'GLM-5.3', total: 200000000, remaining: 100000000, percent_used: 50 },
  ];
  const html = render({ items, plans: [{ name: 'Start Plan', items }] });
  assert.ok(html.includes('1亿/2亿'));
  assert.ok(html.includes('50%'));
});
test('claimed empty plan remains visible while balance is pending from server', () => {
  const html = render({ plans: [{ name: 'ZCode 1024 Dev Day', items: [] }] });
  assert.ok(html.includes('ZCode 1024 Dev Day'));
  assert.ok(html.includes(zh['q.awaitingBalance']));
});
test('single monitor plan keeps its established prompt-window display', () => {
  const items = [
    { name: '提示次数', kind: 'prompt_count', window: 'hours:5', percent_used: 20 },
    { name: '使用时长', kind: 'duration', percent_used: 10 },
  ];
  const html = render({ items, plans: [{ name: 'Coding Plan', items }] });
  assert.ok(html.includes('80%'));
  assert.ok(!html.includes('90%'));
});
