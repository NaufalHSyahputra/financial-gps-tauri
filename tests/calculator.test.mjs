import assert from 'node:assert/strict';
import { test } from 'node:test';
import { calculateMinor } from '../src/lib/calculator.ts';

test('calculator respects precedence, signs, and decimal scales', () => {
  assert.equal(calculateMinor('25000 + 12500 * 2', 0, true), '50000');
  assert.equal(calculateMinor('10 − 4 ÷ 2 × 3', 0, true), '4');
  assert.equal(calculateMinor('0.1 + 0.2', 2, true), '30');
  assert.equal(calculateMinor('1 / 8', 2, true), '13');
  assert.equal(calculateMinor('-1 / 8', 2, true), '-13');
  assert.equal(calculateMinor('4 - -2', 3, true), '6000');
  assert.equal(calculateMinor('12,34', 2, false), '1234');
  assert.equal(calculateMinor('0', 0, false), '0');
});

test('calculator rejects malformed input, division by zero, and unsupported precision', () => {
  assert.throws(() => calculateMinor('1 / 0', 2, true), /divide by zero/);
  for (const input of ['1+', '1//2', 'abc', '1;2', '2e3', '1 2', '.', '1..5']) {
    assert.throws(() => calculateMinor(input, 2, true), undefined, input);
  }
  assert.throws(() => calculateMinor('1+2', 2, false), /Turn on Calculator/);
  assert.throws(() => calculateMinor('1.25', 0, false), /decimal places/);
  assert.throws(() => calculateMinor('999999999999999999', 0, true), /smaller amount/);
});
