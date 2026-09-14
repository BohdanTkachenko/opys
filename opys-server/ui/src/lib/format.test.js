// The rendering helpers that have a contract with the Rust side, run by Node's
// built-in test runner:
//
//     cd opys-server/ui && npm test
//
// Same rules as `flow.test.js`: no test framework and no new dependency, and
// nothing imports this file, so Vite never puts it in the bundle.

import assert from 'node:assert/strict';
import { test } from 'node:test';

import { priorityChip } from './format.js';

// `priorityChip` is `src/commands/next.rs::priority_cell` in the browser. The
// cases below are that function's, in the same order, so a change on either
// side that stopped agreeing shows up here — the flow view's whole claim is
// that it and `opys next --status <s>` cannot disagree, and a chip printing the
// document's own number beside an order computed from the effective one is
// exactly that disagreement.

test('an own priority with nothing inherited shows the number itself', () => {
  const chip = priorityChip(30, 30);
  assert.equal(chip.rank, 30);
  assert.equal(chip.note, null);
  assert.match(chip.title, /^priority 30 — lower is higher$/);
});

test('an inherited rank shows the effective number and the document its own', () => {
  // ADR-0099 decision 4: a task at 30 holding up one at 1 sorts as 1, so 1 is
  // the number that ordered it and 30 is the footnote — not the other way up.
  const chip = priorityChip(30, 1);
  assert.equal(chip.rank, 1);
  assert.equal(chip.note, 'own 30');
  assert.match(chip.title, /effective priority 1\b/);
  assert.match(chip.title, /its own is 30/);
});

test('a document with no priority of its own still shows the rank it inherited', () => {
  // The guard used to be on the own value, so this item drew no chip at all
  // while sitting at the top of its node.
  const chip = priorityChip(null, 1);
  assert.equal(chip.rank, 1);
  assert.equal(chip.note, 'inherited');
  assert.match(chip.title, /inherited/);
});

test('no priority anywhere in the closure draws no chip', () => {
  assert.equal(priorityChip(null, null), null);
  assert.equal(priorityChip(undefined, undefined), null);
});

test('zero is a priority like any other, and the highest one there is', () => {
  const chip = priorityChip(0, 0);
  assert.notEqual(chip, null);
  assert.equal(chip.rank, 0);
  assert.equal(chip.note, null);
});

test('a non-numeric priority is not a rank', () => {
  assert.equal(priorityChip('high', null), null);
  // …and one bad half does not poison the other.
  assert.equal(priorityChip('high', 4).rank, 4);
  assert.equal(priorityChip('high', 4).note, 'inherited');
});
