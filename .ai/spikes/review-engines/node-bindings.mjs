import { JSDOM } from 'jsdom';
import { writeFileSync } from 'node:fs';
const dom = new JSDOM('<!doctype html><html><body></body></html>', { pretendToBeVisual: true });
for (const key of ['window','document','navigator','Node','HTMLElement','MutationObserver','getComputedStyle']) {
  Object.defineProperty(globalThis,key,{value:key==='getComputedStyle'?dom.window[key].bind(dom.window):dom.window[key],configurable:true});
}
globalThis.requestAnimationFrame=dom.window.requestAnimationFrame.bind(dom.window);
globalThis.cancelAnimationFrame=dom.window.cancelAnimationFrame.bind(dom.window);
const { runBindingCases } = await import('./binding-cases.mjs');
const results = await runBindingCases(r=>console.log(`${r.pass?'PASS':'MISS'} ${r.engine}: ${r.case} [${r.delivery}] ${r.error?r.error.split('\n')[0]:JSON.stringify(r.quotes)}`));
writeFileSync(new URL('binding-results.json',import.meta.url),JSON.stringify({runtime:process.version,environment:'jsdom; real upstream EditorView plugins',results},null,2)+'\n');
console.log(JSON.stringify({cases:results.length,passes:results.filter(x=>x.pass).length,misses:results.filter(x=>!x.pass).length,errors:results.filter(x=>x.error).length}));
dom.window.close();
