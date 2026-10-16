import { test } from 'node:test';
import assert from 'node:assert/strict';
import { diffBlocks, diffWords } from '../src/diff.js';

const shape = (ops) => ops.map((o) => o.type);

test('identical documents are all same', () => {
  const doc = '# T\n\npara\n\n- a\n- b\n';
  assert.deepEqual(shape(diffBlocks(doc, doc)), ['same', 'same', 'same', 'same']);
});

test('an edited block is changed, an inserted one added, a dropped one removed', () => {
  const before = '# T\n\none\n\ntwo\n\nthree\n';
  const after = '# T\n\none, edited\n\ntwo\n\nnew\n\nthree\n';
  assert.deepEqual(diffBlocks(before, after), [
    { type: 'same', before: '# T', after: '# T' },
    { type: 'changed', before: 'one', after: 'one, edited' },
    { type: 'same', before: 'two', after: 'two' },
    { type: 'added', after: 'new' },
    { type: 'same', before: 'three', after: 'three' },
  ]);
  assert.deepEqual(shape(diffBlocks('a\n\nb\n\nc\n', 'a\n\nc\n')), ['same', 'removed', 'same']);
});

test('frontmatter is a block like any other', () => {
  const before = '---\nstatus: todo\n---\n\nbody\n';
  const after = '---\nstatus: done\n---\n\nbody\n';
  assert.deepEqual(shape(diffBlocks(before, after)), ['changed', 'same']);
});

test('a word diff keeps the shared words and marks the rest', () => {
  assert.deepEqual(diffWords('the quick fox', 'the slow fox'), [
    { type: 'same', text: 'the ' },
    { type: 'del', text: 'quick' },
    { type: 'add', text: 'slow' },
    { type: 'same', text: ' fox' },
  ]);
  // Reassembling either side gives it back.
  const ops = diffWords('- [ ] item one', '- [x] item two!');
  assert.equal(ops.filter((o) => o.type !== 'add').map((o) => o.text).join(''), '- [ ] item one');
  assert.equal(ops.filter((o) => o.type !== 'del').map((o) => o.text).join(''), '- [x] item two!');
});

test('unrelated blocks at one spot are a removal and an addition', () => {
  assert.deepEqual(shape(diffBlocks('a\n\nRemoved at the end.\n', 'a\n\nA new closing paragraph.\n')), [
    'same',
    'removed',
    'added',
  ]);
});

test('whitespace between changed words joins them into one change', () => {
  assert.deepEqual(diffWords('x Removed at the end.', 'x A new closing para.'), [
    { type: 'same', text: 'x ' },
    { type: 'del', text: 'Removed at the end' },
    { type: 'add', text: 'A new closing para' },
    { type: 'same', text: '.' },
  ]);
});
