import assert from 'node:assert/strict';
import { writeFileSync, mkdirSync } from 'node:fs';
import * as A from '@automerge/automerge';
import * as Y from 'yjs';

const results=[];
function test(engine,name,fn) {
  try { const evidence=fn(); results.push({engine,case:name,pass:true,evidence}); }
  catch(error) { results.push({engine,case:name,pass:false,error:error.stack}); }
}
const aRange=(d,path,start,end)=>({start:A.getCursor(d,path,start,'after'),end:A.getCursor(d,path,end,'before')});
const aQuote=(d,path,r)=> {
  const start=A.getCursorPosition(d,path,r.start),end=A.getCursorPosition(d,path,r.end);
  const text=path.reduce((v,k)=>v[k],d); return text.slice(start,end);
};
const yRange=(t,start,end)=>({start:Y.relativePositionToJSON(Y.createRelativePositionFromTypeIndex(t,start,0)),
  end:Y.relativePositionToJSON(Y.createRelativePositionFromTypeIndex(t,end,-1))});
const yQuote=(d,r)=> {
  // Yjs lazily materializes root types after loading binary state.
  if(d.share.has('text'))d.getText('text');
  const start=Y.createAbsolutePositionFromRelativePosition(Y.createRelativePositionFromJSON(r.start),d);
  const end=Y.createAbsolutePositionFromRelativePosition(Y.createRelativePositionFromJSON(r.end),d);
  return start&&end&&start.type===end.type?start.type.toString().slice(start.index,end.index):null;
};
function yClone(d,id) {const copy=new Y.Doc();Y.applyUpdate(copy,Y.encodeStateAsUpdate(d));copy.clientID=id;return copy;}

for(const boundary of ['start','end']) {
  test('Automerge','cursor-pair boundary '+boundary,()=>{
    let d=A.from({text:'prefix TARGET suffix'}),r=aRange(d,['text'],7,13);
    d=A.change(d,x=>A.splice(x,['text'],boundary==='start'?7:13,0,'+'));
    const quote=aQuote(d,['text'],r);
    assert.equal(quote,boundary==='start'?'TARGET':'TARGET+');
    return {quote,desired:'TARGET',meetsExclusiveBoundary:quote==='TARGET',note:'Cursor move option concerns deletion, not insertion affinity.'};
  });
  test('Automerge','native non-expanding mark boundary '+boundary,()=>{
    let d=A.from({text:'prefix TARGET suffix',threads:{c1:{quote:'TARGET'}}});
    d=A.change(d,x=>A.mark(x,['text'],{start:7,end:13,expand:'none'},'review:c1',true));
    d=A.change(d,x=>A.splice(x,['text'],boundary==='start'?7:13,0,'+'));
    const mark=A.marks(d,['text']).find(m=>m.name==='review:c1');
    const quote=d.text.slice(mark.start,mark.end);assert.equal(quote,'TARGET');return {quote};
  });
  test('Yjs','relative-position boundary '+boundary,()=>{
    const d=new Y.Doc(),t=d.getText('text');t.insert(0,'prefix TARGET suffix');const r=yRange(t,7,13);
    t.insert(boundary==='start'?7:13,'+');const quote=yQuote(d,r);assert.equal(quote,'TARGET');return {quote};
  });
}

test('Automerge','stable block object: move races edit and comment',()=>{
  const seed=A.from({blocks:{a:{body:'See TARGET here.',order:1},b:{body:'Second paragraph.',order:2}},threads:{}});
  let left=A.clone(seed),right=A.clone(seed);
  left=A.change(left,d=>{d.blocks.a.order=3;});
  const target={block:'a',...aRange(right,['blocks','a','body'],4,10)};
  right=A.change(right,d=>{d.threads.c1=target;A.splice(d,['blocks','a','body'],7,0,'!');});
  const merged=A.merge(left,right),quote=aQuote(merged,['blocks','a','body'],merged.threads.c1);
  const order=Object.entries(merged.blocks).sort((a,b)=>a[1].order-b[1].order).map(([id])=>id);
  assert.equal(quote,'TAR!GET');assert.deepEqual(order,['b','a']);return {quote,order,scope:'One mover; no editor binding or general concurrent tree moves.'};
});
test('Yjs','stable block object: move races edit and comment',()=>{
  const seed=new Y.Doc(),blocks=seed.getMap('blocks');
  for(const [id,body,order] of [['a','See TARGET here.',1],['b','Second paragraph.',2]]) {
    const block=new Y.Map();blocks.set(id,block);block.set('body',new Y.Text(body));block.set('order',order);
  }
  const left=yClone(seed,101),right=yClone(seed,102);
  left.getMap('blocks').get('a').set('order',3);
  const body=right.getMap('blocks').get('a').get('body');
  right.getMap('threads').set('c1',{block:'a',...yRange(body,4,10)});body.insert(7,'!');
  Y.applyUpdate(left,Y.encodeStateAsUpdate(right));
  const quote=yQuote(left,left.getMap('threads').get('c1'));
  const order=[...left.getMap('blocks')].sort((a,b)=>a[1].get('order')-b[1].get('order')).map(([id])=>id);
  assert.equal(quote,'TAR!GET');assert.deepEqual(order,['b','a']);return {quote,order,scope:'One mover; requires a different document model and editor integration.'};
});

test('Automerge','two offline accept commands duplicate proposal text',()=>{
  const seed=A.from({text:'',proposal:{status:'open',text:'APPROVED'}});
  const accept=doc=>A.change(doc,d=>{if(d.proposal.status==='open'){A.splice(d,['text'],0,0,d.proposal.text);d.proposal.status='accepted';}});
  const merged=A.merge(accept(A.clone(seed)),accept(A.clone(seed)));
  assert.equal(merged.text,'APPROVEDAPPROVED');assert.equal(merged.proposal.status,'accepted');
  return {text:merged.text,status:merged.proposal.status,meetsExactlyOnce:false};
});
test('Yjs','two offline accept commands duplicate proposal text',()=>{
  const seed=new Y.Doc();seed.getMap('proposal').set('status','open');
  const left=yClone(seed,101),right=yClone(seed,102);
  for(const d of [left,right])d.transact(()=>{if(d.getMap('proposal').get('status')==='open'){d.getText('text').insert(0,'APPROVED');d.getMap('proposal').set('status','accepted');}});
  Y.applyUpdate(left,Y.encodeStateAsUpdate(right));
  assert.equal(left.getText('text').toString(),'APPROVEDAPPROVED');
  return {text:left.getText('text').toString(),status:left.getMap('proposal').get('status'),meetsExactlyOnce:false};
});
test('Authority (usable with all three)','serialized decision and request deduplication',()=>{
  const state={text:'',status:'open',requests:new Map()};
  function accept(requestId) {
    if(state.requests.has(requestId))return state.requests.get(requestId);
    const result=state.status==='open'?'accepted':'already-decided';
    if(result==='accepted'){state.text+='APPROVED';state.status='accepted';}
    state.requests.set(requestId,result);return result;
  }
  const responses=[accept('request-a'),accept('request-b'),accept('request-a')];
  assert.equal(state.text,'APPROVED');assert.deepEqual(responses,['accepted','already-decided','accepted']);
  return {text:state.text,responses,scope:'Single authority transaction; durable storage/crash recovery not implemented.'};
});

test('Automerge','binary restart accepts delayed comment created before restart',()=>{
  const seed=A.from({text:'prefix TARGET suffix',threads:{}});let left=A.clone(seed),right=A.clone(seed);
  const r=aRange(right,['text'],7,13);right=A.change(right,d=>{d.threads.c1=r;});
  left=A.change(left,d=>A.splice(d,['text'],0,0,'Earlier '));
  const restored=A.merge(A.load(A.save(left)),right),quote=aQuote(restored,['text'],restored.threads.c1);
  assert.equal(quote,'TARGET');return {quote};
});
test('Yjs','binary restart accepts delayed comment created before restart',()=>{
  const seed=new Y.Doc();seed.getText('text').insert(0,'prefix TARGET suffix');
  const left=yClone(seed,101),right=yClone(seed,102);
  right.getMap('threads').set('c1',yRange(right.getText('text'),7,13));left.getText('text').insert(0,'Earlier ');
  const restored=yClone(left,103);Y.applyUpdate(restored,Y.encodeStateAsUpdate(right));
  const quote=yQuote(restored,restored.getMap('threads').get('c1'));assert.equal(quote,'TARGET');return {quote};
});
test('Automerge','JSON reconstruction does not retain cursor identity',()=>{
  let d=A.from({text:'prefix TARGET suffix',threads:{}});const target=aRange(d,['text'],7,13);
  d=A.change(d,x=>{x.threads.c1=target;});const rebuilt=A.from(JSON.parse(JSON.stringify(d)));
  let quote=null,error=null;try{quote=aQuote(rebuilt,['text'],rebuilt.threads.c1);}catch(e){error=e.message;}
  assert.notEqual(quote,'TARGET');return {quote,error};
});
test('Yjs','JSON reconstruction does not retain cursor identity',()=>{
  const d=new Y.Doc(),t=d.getText('text');t.insert(0,'prefix TARGET suffix');const target=yRange(t,7,13);
  const rebuilt=new Y.Doc();rebuilt.getText('text').insert(0,t.toString());
  const quote=yQuote(rebuilt,target);assert.equal(quote,null);return {quote};
});

// Native Rust loads exactly these bytes and resolves these JavaScript anchors.
mkdirSync(new URL('fixtures/',import.meta.url),{recursive:true});
const unicodeText='😀 e\u0301 prefix TARGET suffix';
let ad=A.from({text:unicodeText,threads:{}});const start=unicodeText.indexOf('TARGET'),end=start+6;
const ar=aRange(ad,['text'],start,end);
ad=A.change(ad,d=>{d.threads.c1=ar;A.mark(d,['text'],{start,end,expand:'none'},'bold',true);});
writeFileSync(new URL('fixtures/automerge-js.bin',import.meta.url),A.save(ad));
const yd=new Y.Doc(),yt=yd.getText('text');yt.insert(0,unicodeText);yt.format(start,6,{bold:true});
const yr=yRange(yt,start,end);yd.getMap('threads').set('c1',yr);
writeFileSync(new URL('fixtures/yjs-js.bin',import.meta.url),Y.encodeStateAsUpdate(yd));
writeFileSync(new URL('fixtures/anchors.json',import.meta.url),JSON.stringify({text:unicodeText,start,end,automerge:ar,
  yjs:{start:[...Y.encodeRelativePosition(Y.createRelativePositionFromJSON(yr.start))],end:[...Y.encodeRelativePosition(Y.createRelativePositionFromJSON(yr.end))]}}));
writeFileSync(new URL('core-results.json',import.meta.url),JSON.stringify({runtime:process.version,results},null,2)+'\n');
for(const r of results)console.log(`${r.pass?'PASS':'FAIL'} ${r.engine}: ${r.case} ${JSON.stringify(r.evidence??r.error)}`);
if(results.some(r=>!r.pass))process.exitCode=1;
