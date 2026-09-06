import {spawnSync} from 'node:child_process';
import {readFileSync,writeFileSync} from 'node:fs';
import assert from 'node:assert/strict';
const cwd=new URL('.',import.meta.url);let log='';
for(const [command,...args]of [
  ['node','suite.mjs'],['node','node-bindings.mjs'],['node','slate-cases.mjs'],['node','central-restart.mjs'],
  ['cargo','run','--locked','--offline','--manifest-path','rust/Cargo.toml'],['node','verify-interop.mjs'],['python3','baseline.py']]){
  const run=spawnSync(command,args,{cwd,encoding:'utf8'});
  log+=`\n${command} ${args.join(' ')}\n${run.stdout??''}${run.stderr??''}`;
  writeFileSync(new URL('verification-run.log',cwd),log);
  if(run.status!==0){console.error(run.stdout,run.stderr,run.error??'');process.exit(run.status??1);}
  console.log('Verified: '+command+' '+args.join(' '));
}
const read=name=>JSON.parse(readFileSync(new URL(name,cwd)));
const binding=read('binding-results.json').results;
assert.equal(binding.length,180);
// Lock the observed counterexamples as well as the successful cases. A future
// upstream change that fixes a miss must change this evidence intentionally.
for(const r of binding){
  assert.equal(r.error,undefined);
  const expectedMiss=r.case.startsWith('delete and reinsert')||
    (r.engine==='Automerge + ProseMirror'&&/^(link|heading)/.test(r.case))||
    (r.engine==='Yjs + ProseMirror'&&/^(list wrapping|heading|paragraph split)/.test(r.case));
  assert.equal(r.pass,!expectedMiss,JSON.stringify({engine:r.engine,case:r.case,fixture:r.fixture}));
}
const slate=read('slate-results.json').results;assert.equal(slate.length,56);
for(const r of slate){assert.equal(r.error,undefined);assert.equal(r.pass,!/^(paragraph split|Slate move_node|move cannot)/.test(r.case),r.case);}
assert.equal(read('core-results.json').results.length,15);
assert.ok(read('core-results.json').results.every(r=>r.pass));
console.log('Observed outcomes verified: 180 ProseMirror binding runs, 56 Slate binding runs, 15 core checks, restart, native interop, and production-code counterexamples.');
console.log('Known MISS results are reproduced limitations, not passing product requirements.');
