import { test } from 'node:test';
import assert from 'node:assert/strict';
import { documentProblems, verificationFindings, sourceLines, editorDiagnostics } from './verification.js';

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

test('structured document identity and duplicate messages retain their own ranges', () => {
  const own = [1, 2].map((line) => ({message:'same message', doc_id:'ADR-0010', range:{start:{line,column:1},end:{line,column:2}}}));
  const report = {findings:[...own,{message:'mentions ADR-0010',doc_id:'ADR-0011'}]};
  assert.deepEqual(verificationFindings(report,'ADR-0010','inventory/ADR-0010.md'),own);
  assert.deepEqual(verificationFindings({problems:['ADR-0010: old server']},'ADR-0010'),[{message:'ADR-0010: old server'}]);
});

test('source highlighting uses scalar columns and excludes an end at the next line', () => {
  const finding = {range:{start:{line:4,column:5},end:{line:5,column:1}},excerpt:{first_line:3,text:'context\nλ 🦀 broken\nnext'}};
  const lines = sourceLines(finding);
  assert.equal(lines[0].selected,false);
  assert.equal(lines[1].before,'λ 🦀 ');
  assert.equal(lines[1].marked,'broken');
  assert.equal(lines[2].selected,false);
  const point=sourceLines({...finding,range:{start:{line:4,column:5},end:{line:4,column:5}}})[1];
  assert.equal(point.point,true);
  assert.equal(point.after,'broken');
});

test('editor diagnostics convert Unicode columns to textarea offsets and refuse stale snapshots', () => {
  const body='# Title\n\nλ 🦀 broken\n\nLast paragraph\n';
  const finding={message:'invalid text',body_version:'v1',body_range:{start:{line:3,column:5},end:{line:3,column:11}}};
  const [range]=editorDiagnostics(body,'v1',[finding]);
  assert.equal(body.slice(range.start,range.end),'broken');
  assert.equal(range.message,'invalid text');
  assert.deepEqual(editorDiagnostics(body,'v2',[finding]),[]);
  const [point]=editorDiagnostics(body,'v1',[{...finding,body_range:{start:{line:6,column:1},end:{line:6,column:1}}}]);
  assert.equal(point.start,body.length);
  assert.equal(point.end,body.length);
});
