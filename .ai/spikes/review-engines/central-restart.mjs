import {JSDOM} from 'jsdom';
import assert from 'node:assert/strict';
import {writeFileSync} from 'node:fs';
const dom=new JSDOM('<!doctype html><body></body>',{pretendToBeVisual:true});
for(const key of ['window','document','navigator','Node','HTMLElement','MutationObserver'])Object.defineProperty(globalThis,key,{value:dom.window[key],configurable:true});
const {CentralEditor,documentOf,textRange}=await import('./editor-engines.mjs');
const results=[];
for(const retainHistory of [false,true]){
  const [a,b]=CentralEditor.pair(documentOf(['See TARGET here.']));
  const r=textRange(b,'TARGET');b.comment('c1',r.from,r.to);b.view.dispatch(b.view.state.tr.insertText('!',r.from+3));
  a.view.dispatch(a.view.state.tr.insertText('Earlier ',1));
  let bytes=a.save();
  if(!retainHistory){const stored=JSON.parse(new TextDecoder().decode(bytes));delete stored.steps;delete stored.clients;bytes=new TextEncoder().encode(JSON.stringify(stored));}
  const restored=CentralEditor.restore(bytes);b.authority=restored.authority;
  CentralEditor.sync([restored,b]);
  const quote=b.comments()[0].quote,text=b.view.state.doc.textContent;
  const correct=quote==='TAR!GET'&&text==='Earlier See TAR!GET here.';
  assert.equal(correct,retainHistory);
  results.push({retainHistory,quote,text,correct,note:retainHistory?'Old client rebased through persisted steps':'Resetting version and discarding steps falsely accepts stale positions'});
  [a,b,restored].forEach(e=>e.dispose());
}
writeFileSync(new URL('central-restart-results.json',import.meta.url),JSON.stringify({results},null,2)+'\n');
console.log(JSON.stringify(results));dom.window.close();
