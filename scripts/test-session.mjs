import assert from 'node:assert/strict';
import { getReconnectDecision, RECONNECT_POLICY } from '../src/components/reconnectPolicy.ts';

const expectedDelays = [250, 500, 1000, 2000];
let attempts = 0;

for (const expectedDelay of expectedDelays) {
  const decision = getReconnectDecision(attempts, 0);
  assert.ok(decision);
  assert.equal(decision.delayMs, expectedDelay);
  attempts = decision.attempt;
}

assert.equal(getReconnectDecision(attempts, 0), null);
assert.deepEqual(
  getReconnectDecision(attempts, RECONNECT_POLICY.stableSessionMs),
  { attempt: 1, delayMs: RECONNECT_POLICY.baseDelayMs },
);
assert.deepEqual(getReconnectDecision(-10, 0), {
  attempt: 1,
  delayMs: RECONNECT_POLICY.baseDelayMs,
});

console.log('session policy: 7/7 checks passed');
