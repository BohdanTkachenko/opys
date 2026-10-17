import { test } from 'node:test';
import assert from 'node:assert/strict';
import { documentProblems } from './verification.js';

test('a card includes ID and path findings, without errors merely referencing it', () => {
  const own = [
    'ADR-0010: missing history',
    '/home/dan/project/inventory/ADR-0010.md: filename does not match id ADR-0010',
    'inventory/ADR-0010.md: duplicate id ADR-0010',
  ];
  const all = [...own, 'ADR-0011: unresolved reference to ADR-0010', 'opys.toml: invalid config'];
  assert.deepEqual(documentProblems(all, 'ADR-0010', 'inventory/ADR-0010.md'), own);
  assert.deepEqual(documentProblems(all, null, null), all);
});
