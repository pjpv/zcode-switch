// One receipt belongs to one request, even if the next request uses the same account.
export function createClaimResultWaiter({ accountId, claimId, timeoutMs = 90000,
  setTimer = setTimeout, clearTimer = clearTimeout }) {
  let done = false, resolve;
  const promise = new Promise(r => { resolve = r; });
  const timer = setTimer(() => finish(null), timeoutMs);
  function finish(value) {
    if (done) return false;
    done = true;
    clearTimer(timer);
    resolve(value);
    return true;
  }
  return {
    accountId, claimId, promise, finish,
    accept: payload => payload?.accountId === accountId && payload?.claimId === claimId
      ? finish(payload) : false,
  };
}
