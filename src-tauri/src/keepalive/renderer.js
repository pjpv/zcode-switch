/* Evaluated only in the controller-verified local ZCode desktop renderer. */
(() => {
  'use strict';
  if (globalThis.__zswitchKeepalive?.contractVersion === 1) return { contractVersion: 1 };
  const own = (o, k) => o && Object.prototype.hasOwnProperty.call(o, k);
  const pathKey = p => p.replace(/\//g, '\\').replace(/\\+$/, '').toLowerCase();
  const valid = t => t && typeof t.sessionId === 'string' && /^[A-Za-z0-9_-]{1,200}$/.test(t.sessionId) && typeof t.workspacePath === 'string' && /^[A-Za-z]:[\\/]/.test(t.workspacePath) && !/[\0\r\n]/.test(t.workspacePath) && !t.workspaceIdentity && !t.remoteSessionId;
  const scope = t => ({ sessionId: t.sessionId, workspacePath: t.workspacePath });
  function discover() {
    const root = document.getElementById('root');
    if (!root) return null;
    const roots = Object.keys(root).filter(k => /^__react(Container|Fiber)\$/.test(k)).map(k => root[k]);
    const stack = roots.map(f => f?.stateNode?.current || f?.current || f);
    const seen = new Set();
    while (stack.length && seen.size < 10000) {
      const f = stack.pop();
      if (!f || seen.has(f)) continue;
      seen.add(f);
      for (const s of [f.memoizedProps?.services, f.memoizedProps?.value]) {
        if (own(s, 'zcodeAgentService') && own(s, 'zcodeTaskService')) return s;
      }
      if (f.sibling) stack.push(f.sibling);
      if (f.child) stack.push(f.child);
    }
    return null;
  }
  const services = discover();
  const agent = services?.zcodeAgentService, tasks = services?.zcodeTaskService;
  const capable = !!agent && ['helloConversationV4','readSession','resumeSession','sendConversationCommandV4'].every(k => typeof agent[k] === 'function') && typeof tasks?.getTaskMeta === 'function';
  async function probe() {
    if (!capable) return { supported: false, reason: 'capability_unavailable' };
    try {
      const clientId = globalThis.localStorage?.getItem('zcode-v4-client-id:v1');
      if (!clientId) return { supported:false, reason:'desktop_client_not_ready' };
      const h = await agent.helloConversationV4();
      return { supported: h?.protocolVersion === 3 && h?.clientMode === 'desktop-continuous', protocolVersion: h?.protocolVersion, clientMode: h?.clientMode };
    } catch { return { supported: false, reason: 'handshake_unavailable' }; }
  }
  async function meta(t) {
    const m = await tasks.getTaskMeta({taskId: t.sessionId, workspacePath: t.workspacePath});
    if (!m || m.taskId !== t.sessionId || typeof m.workspacePath !== 'string' || pathKey(m.workspacePath) !== pathKey(t.workspacePath) || m.workspaceIdentity) throw Error('identity_unavailable');
    return m;
  }
  function summarize(t, s, m) {
    const a = s?.session, p = s?.projection, r = s?.runtime;
    if (a?.sessionId !== t.sessionId || p?.sessionId !== t.sessionId || !a.workspace || a.workspace.workspaceIdentity || a.workspace.remoteSessionId || pathKey(a.workspace.workspacePath || '') !== pathKey(t.workspacePath) || !Array.isArray(p.pendingPermissions) || !Array.isArray(r?.pendingRequestIds)) throw Error('snapshot_unavailable');
    const statuses = ['idle','running','waiting','paused','completed','error'];
    if (!statuses.includes(a.status) || !statuses.includes(p.status)) throw Error('snapshot_unavailable');
    const pendingInput = !!m.pendingInteraction || p.pendingPermissions.length > 0 || r.pendingRequestIds.length > 0 || a.status === 'waiting' || p.status === 'waiting';
    let status = a.status;
    if (a.status === 'paused' || p.status === 'paused') status = 'paused';
    else if (pendingInput) status = 'waiting';
    else if (a.status === 'running' || p.status === 'running' || r.activeTurnId || p.activeToolCalls?.some(x => ['running','pending'].includes(x.status))) status = 'running';
    else if (a.status === 'completed' || p.status === 'completed' || m.status === 'completed') status = 'completed';
    return {...scope(t), exists:true, status, pendingInput, updatedAt:a.updatedAt, turnId:r.activeTurnId || p.currentTurnId || null};
  }
  const read = t => agent.readSession({...scope(t), runtimePolicy:'existing-only', messageLimit:1});
  async function inspect(targets) {
    if (!Array.isArray(targets) || targets.length > 64 || targets.some(t => !valid(t))) throw Error('invalid_targets');
    if (!(await probe()).supported) return targets.map(t => ({...scope(t),exists:false,status:'unsupported',pendingInput:true}));
    const results = [];
    for (const t of targets) {
      try { const m = await meta(t); results.push(summarize(t, await read(t), m)); }
      catch { results.push({...scope(t),exists:false,status:'unavailable',pendingInput:true}); }
    }
    return results;
  }
  // Cache in-flight and uncertain sends too: a lost ACK must never cause a new send.
  const commands = new Map(), busy = new Set();
  async function execute(t) {
    const result = status => ({...scope(t),commandId:t.commandId,status});
    if (!(await probe()).supported) return result('unsupported');
    let m;
    try { m = await meta(t); } catch { return result('identity_unavailable'); }
    if (m.pendingInteraction) return result('blocked');
    if (m.status === 'completed') return result('completed');
    let snapshot;
    try { snapshot = await read(t); }
    catch {
      // Only an explicitly journaled recovery may hydrate the original session.
      if (!['running','error'].includes(m.status)) return result('unavailable');
      try { snapshot = await agent.resumeSession(scope(t)); } catch { return result('unavailable'); }
    }
    let state;
    try { state = summarize(t, snapshot, m); } catch { return result('unavailable'); }
    if (state.pendingInput || state.status === 'paused') return result('blocked');
    if (state.status === 'running') return result('already_running');
    if (state.status === 'completed') return result('completed');
    if (!['idle','error'].includes(state.status)) return result('blocked');
    // A new user turn after detection invalidates the captured recovery target.
    if (t.baselineTurnId && state.turnId && t.baselineTurnId !== state.turnId) return result('changed');
    try {
      const ack = await agent.sendConversationCommandV4({workspacePath:t.workspacePath,envelope:{commandId:t.commandId,clientId:globalThis.localStorage.getItem('zcode-v4-client-id:v1'),sessionId:t.sessionId,type:'sendText',payload:{text:'继续',requestedDelivery:'startNow'},issuedAt:t.issuedAt}});
      return {...result(ack?.commandId === t.commandId && ['accepted','rejected','stale','duplicate','noop','failed'].includes(ack?.status) ? ack.status : 'unknown_ack'), requiresObservation:true};
    } catch { return {...result('uncertain'),requiresObservation:true}; }
  }
  function resume(t) {
    if (!valid(t) || typeof t.commandId !== 'string' || !/^[A-Za-z0-9_-]{1,200}$/.test(t.commandId) || !Number.isFinite(t.issuedAt) || !Number.isFinite(t.recoveryStartedAt) || t.recoveryStartedAt > t.issuedAt) return Promise.resolve({status:'invalid_target'});
    const key = pathKey(t.workspacePath) + '\0' + t.sessionId;
    if (commands.has(t.commandId)) { const c=commands.get(t.commandId); return c.key === key ? c.promise : Promise.resolve({status:'command_scope_mismatch'}); }
    if (busy.has(key)) return Promise.resolve({...scope(t),commandId:t.commandId,status:'busy'});
    if (commands.size >= 2048) return Promise.resolve({status:'command_capacity'});
    busy.add(key);
    const promise = execute(t).then(r => { if (!r.requiresObservation) commands.delete(t.commandId); return r; }).finally(() => busy.delete(key));
    commands.set(t.commandId, {key,promise});
    return promise;
  }
  Object.defineProperty(globalThis,'__zswitchKeepalive',{value:Object.freeze({contractVersion:1,probe,inspect,resume}),configurable:true});
  return {contractVersion:1};
})()
