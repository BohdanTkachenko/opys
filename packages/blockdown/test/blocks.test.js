import { test } from 'node:test';
import assert from 'node:assert/strict';
import { frontmatterHtml, insertBlock, segment, splice, toggleTask } from '../src/blocks.js';
import { locate } from '../src/selection.js';

/** The blocks' texts and kinds, which is what the assertions care about. */
const texts = (src) => segment(src).map((b) => [b.kind, src.slice(b.start, b.end)]);

test('paragraphs, headings and rules are separate blocks', () => {
  const src = '# Title\n\nFirst para\nstill first.\n\n## Section\n---\nAfter rule.\n';
  assert.deepEqual(texts(src), [
    ['heading', '# Title'],
    ['paragraph', 'First para\nstill first.'],
    ['heading', '## Section'],
    ['rule', '---'],
    ['paragraph', 'After rule.'],
  ]);
});

test('a setext underline ends its paragraph as a heading', () => {
  assert.deepEqual(texts('Title\n=====\nbody\n'), [
    ['heading', 'Title\n====='],
    ['paragraph', 'body'],
  ]);
});

test('a fence owns its blank lines and its markdown-looking lines', () => {
  const src = 'before\n\n```js\nconst a = 1;\n\n# not a heading\n```\nafter\n';
  assert.deepEqual(texts(src), [
    ['paragraph', 'before'],
    ['code', '```js\nconst a = 1;\n\n# not a heading\n```'],
    ['paragraph', 'after'],
  ]);
});

test('an unclosed fence runs to the end', () => {
  assert.deepEqual(texts('~~~\ncode\n'), [['code', '~~~\ncode']]);
});

test('each top-level list item is a block, with its nested content', () => {
  const src = '- [ ] one\n  continued\n- [x] two\n  - nested\n\n  loose para\n- three\nlazy\n\nafter\n';
  assert.deepEqual(texts(src), [
    ['list', '- [ ] one\n  continued'],
    ['list', '- [x] two\n  - nested\n\n  loose para'],
    ['list', '- three\nlazy'],
    ['paragraph', 'after'],
  ]);
});

test('ordered items, and only "1." interrupts a paragraph', () => {
  assert.deepEqual(texts('text\n2. not a list\n\n1. a\n2. b\n'), [
    ['paragraph', 'text\n2. not a list'],
    ['list', '1. a'],
    ['list', '2. b'],
  ]);
});

test('quotes, tables, and html comments', () => {
  const src = '> quoted\nlazy\n\n| a | b |\n|---|:-:|\n| 1 | 2 |\n\n<!-- a\ncomment -->\ntext\n';
  assert.deepEqual(texts(src), [
    ['quote', '> quoted\nlazy'],
    ['table', '| a | b |\n|---|:-:|\n| 1 | 2 |'],
    ['html', '<!-- a\ncomment -->'],
    ['paragraph', 'text'],
  ]);
});

test('ranges exclude the line break, and splicing touches nothing else', () => {
  const src = 'a  \n\n\n- x\n-  y\n\n\tcode?\n';
  for (const b of segment(src)) {
    const out = splice(src, b.start, b.end, src.slice(b.start, b.end));
    assert.equal(out, src);
    assert.ok(!src.slice(b.start, b.end).endsWith('\n'));
  }
});

test('emptying a block removes it and its gap', () => {
  const src = 'A\n\nB\n\nC\n';
  const [, b, c] = segment(src);
  assert.equal(splice(src, b.start, b.end, ''), 'A\n\nC\n');
  assert.equal(splice(src, c.start, c.end, '  '), 'A\n\nB\n');
  assert.equal(splice('only\n', 0, 4, ''), '');
});

test('inserting a block keeps blank-line separation', () => {
  const src = 'A\n\nB\n';
  const [a, b] = segment(src);
  assert.equal(insertBlock(src, a.end, 'new'), 'A\n\nnew\n\nB\n');
  assert.equal(insertBlock(src, b.end, 'new'), 'A\n\nB\n\nnew\n');
  assert.equal(insertBlock(src, 0, 'top'), 'top\n\nA\n\nB\n');
  assert.equal(insertBlock('', 0, 'first'), 'first\n');
  assert.equal(insertBlock(src, a.end, ''), src);
});

test('toggleTask flips the nth checkbox only', () => {
  const src = '- [ ] a\n- [x] b\n  - [ ] c\n- not a task [ ]\n';
  assert.equal(toggleTask(src, 0), '- [x] a\n- [x] b\n  - [ ] c\n- not a task [ ]\n');
  assert.equal(toggleTask(src, 1), '- [ ] a\n- [ ] b\n  - [ ] c\n- not a task [ ]\n');
  assert.equal(toggleTask(src, 2), '- [ ] a\n- [x] b\n  - [x] c\n- not a task [ ]\n');
  assert.equal(toggleTask(src, 3), src);
});

test('locate maps a rendered selection back to its source', () => {
  const source = 'The **bold** word and the word again.';
  const rendered = 'The bold word and the word again.';
  // The second "word", selected in the rendered text.
  const at = rendered.lastIndexOf('word');
  const found = locate(source, rendered, 'word', at);
  assert.equal(source.slice(found.start, found.end), 'word');
  assert.equal(found.start, source.lastIndexOf('word'));
  // Across markup there is no verbatim match.
  assert.equal(locate(source, rendered, 'The bold', 0), null);
  // Whitespace is loose: a source line break matches a rendered space.
  const wrapped = locate('one\ntwo', 'one two', 'one two', 0);
  assert.deepEqual(wrapped, { start: 0, end: 7 });
});

test('a leading frontmatter fence is one block; elsewhere --- is a rule', () => {
  const src = '---\nid: X-1\ntags: [a]\nrefs:\n  A-1: one\n---\n\n# Title\n\n---\n';
  assert.deepEqual(texts(src), [
    ['frontmatter', '---\nid: X-1\ntags: [a]\nrefs:\n  A-1: one\n---'],
    ['heading', '# Title'],
    ['rule', '---'],
  ]);
  // Unclosed: not frontmatter, just a rule and a paragraph.
  assert.deepEqual(texts('---\nid: 1\n')[0], ['rule', '---']);
});

test('frontmatter renders as a key/value table, escaped', () => {
  const html = frontmatterHtml('---\nid: X-1\nnote: <b>\nrefs:\n  A-1: one\n---');
  assert.match(html, /<th>id<\/th><td>X-1<\/td>/);
  assert.match(html, /<td>&lt;b&gt;<\/td>/);
  assert.match(html, /<th>refs<\/th><td>  A-1: one<\/td>/);
  assert.match(frontmatterHtml('---\n# a comment\n---'), /<pre class="bd-frontmatter">/);
});
