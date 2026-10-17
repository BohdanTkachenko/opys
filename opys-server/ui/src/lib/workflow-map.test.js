import assert from 'node:assert/strict';
import { test } from 'node:test';
import { buildWorkflowMap, layoutWorkflowMap, roundedRoute } from './workflow-map.js';

const type = {name:'task',statuses:['todo','doing','review','blocked','done'],default_status:'todo',terminal_statuses:['done'],has_graph:false};
const view = {types:[type],nodes:[],edges:[]};

test('an unrestricted workflow compresses its permissions to one wildcard rule', () => {
  const model = buildWorkflowMap(view, 'task');
  assert.equal(model.nodes.filter(node=>node.kind==='rule').length, 1);
  assert.equal(model.edges.length, 5);
  assert.ok(model.edges.every(edge=>edge.from==='any-active' && edge.wildcard));
  assert.equal(model.edges.filter(edge=>edge.closing).length, 1);
  assert.ok(model.nodes.find(node=>node.status==='todo').initial);
});

test('explicit cyclic workflows preserve only the configured transitions', async () => {
  const explicit = {types:[{...type,has_graph:true}],nodes:[],edges:[
    {from:'todo',to:'doing',types:['task']},{from:'doing',to:'review',types:['task']},
    {from:'review',to:'doing',types:['task']},
    {from:'review',to:'done',types:['task']},{from:'todo',to:'done',types:['chore']},
  ]};
  const model = buildWorkflowMap(explicit, 'task');
  assert.ok(model.nodes.every(node=>node.kind==='status'));
  assert.equal(model.edges.length, 4);
  const result = await layoutWorkflowMap(model);
  assert.equal(result.edges.length, 4);
  assert.ok(result.edges.every(edge=>edge.paths.length && edge.paths.every(path=>!path.includes('NaN'))));
  const y = (status) => result.nodes.find(node=>node.status===status).y;
  assert.ok(y('todo') < y('doing') && y('doing') < y('review') && y('review') < y('done'));
});

test('ELK places the wildcard graph without overlaps and routes every destination', async () => {
  const model = buildWorkflowMap(view,'task');
  const result = await layoutWorkflowMap(model);
  assert.equal(result.edges.length,5);
  assert.ok(Number.isFinite(result.width) && result.width>0);
  assert.ok(Number.isFinite(result.height) && result.height>0);
  for (const a of result.nodes) for(const b of result.nodes) {
    if(a.id===b.id) continue;
    assert.ok(a.x+a.width<=b.x || b.x+b.width<=a.x || a.y+a.height<=b.y || b.y+b.height<=a.y, `${a.id} overlaps ${b.id}`);
  }
  assert.ok(result.edges.every(edge=>edge.paths.some(path=>path.startsWith('M '))));
});

test('unknown statuses stay inspectable but never inherit wildcard permissions', () => {
  const model=buildWorkflowMap(view,'task',[{type:'task',status:'unknown'},{type:'chore',status:'other'}]);
  const unknown=model.nodes.find(node=>node.status==='unknown');
  assert.equal(unknown.declared,false);
  assert.ok(!model.nodes.some(node=>node.status==='other'));
  assert.ok(!model.edges.some(edge=>edge.to===unknown.id));
  const reordered=buildWorkflowMap({types:[{...type,statuses:[...type.statuses].reverse()}],edges:[]},'task');
  assert.equal(model.nodes.find(node=>node.status==='todo').id,reordered.nodes.find(node=>node.status==='todo').id);
});

test('rounded routes tolerate repeated bend points', () => {
  const path=roundedRoute([{x:0,y:0},{x:10,y:0},{x:10,y:0},{x:10,y:20}]);
  assert.ok(!path.includes('NaN'));
  assert.ok(path.endsWith('L 10 20'));
});
