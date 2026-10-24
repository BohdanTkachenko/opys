import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readRole, writeRole } from './role-memory.js';

test('saved roles are isolated by project and persist separately from the global choice', () => {
  const values = new Map();
  const storage = { getItem: (key) => values.get(key), setItem: (key, value) => values.set(key, value) };
  writeRole('global-reviewer', null, storage);
  assert.equal(readRole('project-a', storage), null, 'no fallback to another scope');
  writeRole('writer', 'project-a', storage);
  writeRole('operator', 'project-b', storage);
  assert.equal(readRole(null, storage), 'global-reviewer');
  assert.equal(readRole('project-a', storage), 'writer');
  assert.equal(readRole('project-b', storage), 'operator');
  assert.equal(readRole('project-c', storage), null);
  writeRole('human', 'project-a', storage);
  assert.equal(readRole('project-b', storage), 'operator');
  assert.equal(readRole('project-a', storage), 'human');
});

test('unavailable browser storage does not break the picker', () => {
  const storage = { getItem() { throw new Error('denied'); }, setItem() { throw new Error('denied'); } };
  assert.equal(readRole('project', storage), null);
  assert.doesNotThrow(() => writeRole('reviewer', 'project', storage));
});
