import assert from 'node:assert/strict';
import { performance } from 'node:perf_hooks';
import { findArabicJoinRanges } from '../src/components/arabicRenderer.ts';

const lineCount = 50_000;
const startedAt = performance.now();
let joinedCodeUnits = 0;

for (let index = 0; index < lineCount; index += 1) {
  const arabic = index % 2 === 0 ? 'مرحبا بالعالم' : 'اكتمل البناء بنجاح';
  const line = `build-${index} | ${arabic} | status=ok`;
  const ranges = findArabicJoinRanges(line);
  assert.equal(ranges.length, 1, `unexpected Arabic ranges at line ${index}`);
  const [start, end] = ranges[0];
  assert.equal(line.slice(start, end), arabic, `Arabic text changed at line ${index}`);
  joinedCodeUnits += end - start;
}

const durationMs = performance.now() - startedAt;
assert.ok(joinedCodeUnits > lineCount, 'stress run did not process Arabic phrases');
assert.ok(
  durationMs < 10_000,
  `50,000-line Arabic stress run exceeded 10 seconds: ${durationMs.toFixed(1)}ms`,
);

console.log(
  `Arabic rendering stress: ${lineCount.toLocaleString('en-US')} mixed lines in ${durationMs.toFixed(1)}ms`,
);
