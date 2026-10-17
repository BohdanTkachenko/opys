import assert from 'node:assert/strict';
import { test } from 'node:test';
import { buildWorkflowMap, fitWorkflowMap, layoutWorkflowMap, roundedRoute } from './workflow-map.js';

const type = {name:'task',statuses:['todo','doing','review','blocked','done'],default_status:'todo',terminal_statuses:['done'],has_graph:false};
const view = {types:[type],nodes:[],edges:[]};

test('unrestricted workflows include every individual allowed transition', () => {
  const model = buildWorkflowMap(view, 'task');
  assert.equal(model.nodes.length, 5);
  assert.ok(model.nodes.every(node=>node.kind==='status'));
  assert.equal(model.edges.length, 16);
  for (const from of type.statuses) for (const to of type.statuses) {
    const present=model.edges.some(edge=>edge.from===`state-${from}` && edge.to===`state-${to}`);
    assert.equal(present,from!=='done' && from!==to,`${from} -> ${to}`);
  }
  assert.equal(model.edges.filter(edge=>edge.closing).length, 4);
});

test('the project graph merges statuses and transitions while preserving each type', () => {
  const milestone={...type,name:'milestone',statuses:['todo','doing','done'],terminal_statuses:[]};
  const project={types:[{...type,statuses:['todo','doing','done']},milestone],edges:[]};
  const model=buildWorkflowMap(project);
  assert.equal(model.types.length,2);
  assert.equal(model.nodes.length,3);
  assert.equal(model.edges.length,6);
  const done=model.nodes.find(node=>node.status==='done');
  assert.deepEqual(done.types,['task','milestone']);
  assert.equal(done.terminal,false,'milestones can remain at done');
  const intoDone=model.edges.find(edge=>edge.from==='state-todo' && edge.to==='state-done');
  assert.deepEqual(intoDone.types,['task','milestone']);
  assert.deepEqual(intoDone.closingTypes,['task']);
  assert.equal(intoDone.closing,false);
  assert.deepEqual(model.edges.find(edge=>edge.from==='state-done' && edge.to==='state-todo').types,['milestone']);
});

test('explicit cyclic workflows preserve only their configured transitions', async () => {
  const explicit = {types:[{...type,has_graph:true}],edges:[
    {from:'todo',to:'doing',types:['task']},{from:'doing',to:'review',types:['task']},
    {from:'review',to:'doing',types:['task']},{from:'review',to:'done',types:['task']},
    {from:'todo',to:'done',types:['chore']},
  ]};
  const model = buildWorkflowMap(explicit, 'task');
  assert.equal(model.edges.length, 4);
  const result = await layoutWorkflowMap(model);
  assert.equal(result.edges.length, 4);
  assert.ok(result.edges.every(edge=>edge.paths.length && edge.paths.every(path=>!path.includes('NaN'))));
  const y = (status) => result.nodes.find(node=>node.status===status).y;
  assert.ok(y('todo') < y('doing') && y('doing') < y('review') && y('review') < y('done'));
});

test('ELK routes every edge of the full project graph without overlapping nodes', async () => {
  const result = await layoutWorkflowMap(buildWorkflowMap(view));
  assert.equal(result.edges.length,16);
  assert.ok(Number.isFinite(result.width) && result.width>0);
  assert.ok(Number.isFinite(result.height) && result.height>0);
  for (const a of result.nodes) for(const b of result.nodes) {
    if(a.id===b.id) continue;
    assert.ok(a.x+a.width<=b.x || b.x+b.width<=a.x || a.y+a.height<=b.y || b.y+b.height<=a.y, `${a.id} overlaps ${b.id}`);
  }
  assert.ok(result.edges.every(edge=>edge.paths.some(path=>path.startsWith('M '))));
});

test('live status changes preserve layout geometry when the schema is unchanged', () => {
  const before=buildWorkflowMap(view,null,[{type:'task',status:'todo'}]);
  const after=buildWorkflowMap(view,null,[{type:'task',status:'review'}]);
  assert.equal(JSON.stringify(before),JSON.stringify(after));
});

test('Fit includes the complete diagram even on narrow screens', () => {
  for (const [width,height,vw,vh] of [[2000,1200,390,500],[300,2000,700,450],[600,500,1000,700]]) {
    const scale=fitWorkflowMap(width,height,vw,vh);
    assert.ok(width*scale<=vw-40+1e-8);
    assert.ok(height*scale<=vh-40+1e-8);
  }
});

test('unknown statuses remain visible without invented permissions', () => {
  const model=buildWorkflowMap(view,'task',[{type:'task',status:'unknown'},{type:'chore',status:'other'}]);
  const unknown=model.nodes.find(node=>node.status==='unknown');
  assert.equal(unknown.declared,false);
  assert.ok(!model.nodes.some(node=>node.status==='other'));
  assert.ok(!model.edges.some(edge=>edge.to===unknown.id || edge.from===unknown.id));
});

test('rounded routes tolerate repeated bend points', () => {
  const path=roundedRoute([{x:0,y:0},{x:10,y:0},{x:10,y:0},{x:10,y:20}]);
  assert.ok(!path.includes('NaN'));
  assert.ok(path.endsWith('L 10 20'));
});
