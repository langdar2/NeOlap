import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { looksLikeInput, normalizeInput } from '../pivot-state.js';

const lines = readFileSync(new URL('./input-contract.tsv', import.meta.url), 'utf8')
  .split('\n')
  .filter((l) => l !== '' && !l.startsWith('#'));

test('the contract file has the expected shape', () => {
  assert.ok(lines.length > 40);
  for (const l of lines) assert.match(l, /\t(ok|err)$/, l);
});

test('the UI pre-check never rejects an input the server accepts', () => {
  const wrong = [];
  for (const line of lines) {
    const [input, verdict] = line.split('\t');
    if (verdict === 'ok' && !looksLikeInput(normalizeInput(input))) wrong.push(input);
  }
  assert.deepEqual(wrong, [], `the UI would block valid input: ${JSON.stringify(wrong)}`);
});

test('the UI pre-check still catches plain nonsense without a round trip', () => {
  for (const nonsense of ['abc', '5x', '--5', '#', '@', '@Actual', '5%%', '+%', '%']) {
    assert.equal(looksLikeInput(normalizeInput(nonsense)), false, nonsense);
  }
});
