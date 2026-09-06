// Core-operation benchmark only. No DOM, networking, disk IO, or review mapping.
import * as A from '@automerge/automerge';
import * as Y from 'yjs';
import {schema} from 'prosemirror-schema-basic';
import {Transform} from 'prosemirror-transform';
import {writeFileSync} from 'node:fs';
import {cpus} from 'node:os';
const edits=500,repeats=6;
const adapters=[
  {name:'Automerge',init:text=>A.from({text}),
    edit:(d,pos)=>A.change(d,x=>A.splice(x,['text'],pos,0,'x')),
    save:d=>A.save(d),load:bytes=>A.load(bytes),read:d=>d.text},
  {name:'Yjs',init:text=>{const d=new Y.Doc();d.getText('text').insert(0,text);return d;},
    edit:(d,pos)=>{d.getText('text').insert(pos,'x');return d;},
    save:d=>Y.encodeStateAsUpdate(d),load:bytes=>{const d=new Y.Doc();Y.applyUpdate(d,bytes);return d;},read:d=>d.getText('text').toString()},
  {name:'Central ProseMirror',init:text=>{const doc=schema.node('doc',null,[schema.node('paragraph',null,schema.text(text))]);return {initial:doc.toJSON(),doc,steps:[]};},
    edit:(d,pos)=>{const tr=new Transform(d.doc).insert(pos+1,schema.text('x'));d.doc=tr.doc;d.steps.push(tr.steps[0].toJSON());return d;},
    // Includes the initial snapshot, current snapshot, and retained steps.
    save:d=>new TextEncoder().encode(JSON.stringify({initial:d.initial,doc:d.doc.toJSON(),steps:d.steps})),
    load:bytes=>{const stored=JSON.parse(new TextDecoder().decode(bytes));return {...stored,doc:schema.nodeFromJSON(stored.doc)};},read:d=>d.doc.textContent},
];
const results=[];
for(const size of [10000,100000])for(const adapter of adapters){
  const runs=[];
  const original='abcdefghijklmnopqrstuvwx '.repeat(Math.ceil(size/25)).slice(0,size);
  for(let repeat=0;repeat<repeats;repeat++){
    let start=performance.now(),doc=adapter.init(original);
    const initMs=performance.now()-start,times=[];let expected=original;
    for(let i=0;i<edits;i++){
      const pos=(i*7919)%(size+i);start=performance.now();doc=adapter.edit(doc,pos);times.push(performance.now()-start);
      expected=expected.slice(0,pos)+'x'+expected.slice(pos);
    }
    start=performance.now();const bytes=adapter.save(doc),saveMs=performance.now()-start;
    start=performance.now();const restored=adapter.load(bytes),loadMs=performance.now()-start;
    if(adapter.read(restored)!==expected)throw new Error(adapter.name+' corrupt benchmark output');
    if(repeat)runs.push({initMs,editMs:times,saveMs,loadMs,bytes:bytes.length});
    doc.destroy?.();restored.destroy?.();
  }
  const percentile=(xs,p)=>[...xs].sort((a,b)=>a-b)[Math.min(xs.length-1,Math.floor(xs.length*p))];
  const editTimes=runs.flatMap(r=>r.editMs);
  const row={engine:adapter.name,characters:size,edits,repeats:repeats-1,
    initMedianMs:percentile(runs.map(r=>r.initMs),.5),editP50Ms:percentile(editTimes,.5),editP95Ms:percentile(editTimes,.95),
    totalEditsMedianMs:percentile(runs.map(r=>r.editMs.reduce((a,b)=>a+b,0)),.5),
    saveMedianMs:percentile(runs.map(r=>r.saveMs),.5),loadMedianMs:percentile(runs.map(r=>r.loadMs),.5),bytes:percentile(runs.map(r=>r.bytes),.5)};
  results.push(row);console.log(JSON.stringify(row));
}
writeFileSync(new URL('benchmark-results.json',import.meta.url),JSON.stringify({runtime:process.version,cpu:cpus()[0].model,
  workload:'500 deterministic single-character insertions at scattered positions, one warmup then five measured runs. Core operations only.',
  storageCaveat:'Automerge retains change history. Yjs full state does not provide equivalent historical snapshots. ProseMirror includes initial/current JSON snapshots and serialized steps. Bytes are not equivalent retention guarantees.',results},null,2)+'\n');
