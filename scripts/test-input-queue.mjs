import assert from 'node:assert/strict';
import {
  MAX_PENDING_INPUT_CODE_UNITS,
  TerminalInputQueue,
} from '../src/components/terminalInputQueue.ts';

let passed = 0;

async function check(name, test) {
  await test();
  passed += 1;
  console.log(`  ✓ ${name}`);
}

console.log('Terminal input queue tests');

await check('batches a burst of individual English keystrokes in order', async () => {
  const writes = [];
  const queue = new TerminalInputQueue(async (sessionId, input) => {
    writes.push({ sessionId, input });
  });

  for (const character of 'claude') queue.enqueue(7, character);
  await queue.whenIdle();

  assert.deepEqual(writes, [{ sessionId: 7, input: 'claude' }]);
});

await check('preserves mixed Arabic and English code points exactly', async () => {
  const writes = [];
  const queue = new TerminalInputQueue(async (_sessionId, input) => {
    writes.push(input);
  });
  const mixed = 'مرحبا بالعالم | Fix tests 123';

  for (const character of mixed) queue.enqueue(11, character);
  await queue.whenIdle();

  assert.equal(writes.join(''), mixed);
});

await check('bounds large paste batches without splitting Unicode pairs', async () => {
  const writes = [];
  const queue = new TerminalInputQueue(async (_sessionId, input) => {
    writes.push(input);
  });
  const largePaste = `${'س'.repeat(MAX_PENDING_INPUT_CODE_UNITS - 1)}🌞${'x'.repeat(257)}`;

  queue.enqueue(12, largePaste);
  await queue.whenIdle();

  assert.equal(writes.join(''), largePaste);
  assert.ok(writes.length >= 2, 'large paste must be split into bounded bridge writes');
  assert.ok(
    writes.every((write) => write.length <= MAX_PENDING_INPUT_CODE_UNITS),
    'no pending bridge write may exceed the configured limit',
  );
  assert.ok(writes.every((write) => !write.includes('\uFFFD')));
});

await check('never starts a later bridge write before the earlier write finishes', async () => {
  const events = [];
  let releaseFirst;
  const firstGate = new Promise((resolve) => {
    releaseFirst = resolve;
  });
  const queue = new TerminalInputQueue(async (_sessionId, input) => {
    events.push(`start:${input}`);
    if (input === 'first') await firstGate;
    events.push(`end:${input}`);
  });

  queue.enqueue(3, 'first');
  queue.flush();
  await Promise.resolve();
  queue.enqueue(3, 'second');
  queue.flush();
  await Promise.resolve();

  assert.deepEqual(events, ['start:first']);
  releaseFirst();
  await queue.whenIdle();
  assert.deepEqual(events, ['start:first', 'end:first', 'start:second', 'end:second']);
});

await check('keeps input from different sessions in separate ordered writes', async () => {
  const writes = [];
  const queue = new TerminalInputQueue(async (sessionId, input) => {
    writes.push([sessionId, input]);
  });

  queue.enqueue(1, 'old');
  queue.enqueue(2, 'new');
  await queue.whenIdle();

  assert.deepEqual(writes, [[1, 'old'], [2, 'new']]);
});

await check('orders Ctrl+C-style operations after pending text', async () => {
  const events = [];
  const queue = new TerminalInputQueue(async (_sessionId, input) => {
    events.push(`write:${input}`);
  });

  queue.enqueue(5, 'running');
  queue.enqueueOperation(5, async () => {
    events.push('interrupt');
  });
  await queue.whenIdle();

  assert.deepEqual(events, ['write:running', 'interrupt']);
});

await check('continues after a failed bridge write', async () => {
  const writes = [];
  const errors = [];
  const queue = new TerminalInputQueue(
    async (_sessionId, input) => {
      if (input === 'bad') throw new Error('bridge failed');
      writes.push(input);
    },
    (error) => errors.push(error),
  );

  queue.enqueue(9, 'bad');
  queue.flush();
  queue.enqueue(9, 'good');
  await queue.whenIdle();

  assert.equal(errors.length, 1);
  assert.deepEqual(writes, ['good']);
});

await check('drops pending input after disposal', async () => {
  const writes = [];
  const queue = new TerminalInputQueue(async (_sessionId, input) => {
    writes.push(input);
  });

  queue.enqueue(4, 'stale');
  queue.dispose();
  await queue.whenIdle();

  assert.deepEqual(writes, []);
});

console.log(`\n${passed}/${passed} input queue checks passed`);
