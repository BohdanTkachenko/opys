import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mapDiagnosticRanges, diagnosticPieces, diagnosticsInBlock, draftSnapshot } from '../src/diagnostics.js';

test('draft snapshots preserve typed whitespace and map existing and inserted fields exactly', () => {
  const source = '# Title\n\nold\n\nTail\n';
  const snapshot = draftSnapshot(source,{start:9,end:12,insert:false},'λ 🦀 new  \n');
  assert.equal(snapshot.source, '# Title\n\nλ 🦀 new  \n\n\nTail\n');
  assert.equal(snapshot.source.slice(snapshot.start,snapshot.end), 'λ 🦀 new  \n');
  for (const [text, at] of [[source,0],[source,7],['',0],['\n',0]]) {
    for (const draft of ['', 'λ 🦀 new  \n']) {
      const snapshot = draftSnapshot(text,{start:at,end:at,insert:true},draft);
      assert.equal(snapshot.source.slice(snapshot.start,snapshot.end),draft);
    }
  }
});

test('diagnostics follow unchanged text and disappear when its text is edited', () => {
  const source='λ 🦀 broken tail';
  const start=source.indexOf('broken');
  const diagnostics=[{start,end:start+6,message:'bad reference'}];
  const before=mapDiagnosticRanges(source,'new '+source,diagnostics);
  assert.equal(('new '+source).slice(before[0].start,before[0].end),'broken');
  assert.deepEqual(mapDiagnosticRanges(source,source+' after',diagnostics),diagnostics);
  assert.deepEqual(mapDiagnosticRanges(source,source.replace('broken','fixed'),diagnostics),[]);
  assert.deepEqual(mapDiagnosticRanges(source,source,diagnostics),diagnostics);
});

test('insertion diagnostics disappear when content is inserted there', () => {
  assert.deepEqual(mapDiagnosticRanges('', 'new content', [{start:0,end:0}]),[]);
  assert.deepEqual(mapDiagnosticRanges('tail','pre tail',[{start:4,end:4}]),[{start:8,end:8}]);
});

test('overlapping highlights keep the exact source once and preserve insertion markers', () => {
  const text='one bad token\n';
  const pieces=diagnosticPieces(text,[{start:4,end:7},{start:6,end:13},{start:14,end:14}]);
  assert.equal(pieces.map(piece=>piece.text).join(''),text);
  assert.equal(pieces.filter(piece=>piece.marked&&!piece.point).map(piece=>piece.text).join(''),'bad token');
  assert.equal(pieces.filter(piece=>piece.point).length,1);
  assert.deepEqual(diagnosticPieces('x',[{start:-1,end:2},{start:0,end:50}]),[{text:'x',marked:false,point:false}]);
});

test('block-local highlights clip multiline diagnostics and retain EOF insertion points', () => {
  const diagnostics=[{start:2,end:20,message:'section invalid'},{start:25,end:25,message:'missing section'}];
  assert.deepEqual(diagnosticsInBlock(diagnostics,10,15),[{start:0,end:5,message:'section invalid'}]);
  assert.deepEqual(diagnosticsInBlock(diagnostics,25,25),[{start:0,end:0,message:'missing section'}]);
});
