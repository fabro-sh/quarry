import assert from 'node:assert/strict';
import {readFileSync,writeFileSync} from 'node:fs';
import * as A from '@automerge/automerge';
import * as Y from 'yjs';
const file=name=>new URL('fixtures/'+name,import.meta.url);
const ad=A.load(readFileSync(file('automerge-rust.bin'))),at=ad.threads.c1;
const astart=A.getCursorPosition(ad,['text'],at.start),aend=A.getCursorPosition(ad,['text'],at.end);
assert.equal(ad.text.slice(astart,aend),'TAR!GET');
assert.deepEqual(A.marks(ad,['text']).map(m=>({name:m.name,quote:ad.text.slice(m.start,m.end)})),[{name:'bold',quote:'TAR!GET'}]);
const yd=new Y.Doc(),yt=yd.getText('text');Y.applyUpdate(yd,readFileSync(file('yjs-rust.bin')));
const target=yd.getMap('threads').get('c1');
const ys=Y.createAbsolutePositionFromRelativePosition(Y.createRelativePositionFromJSON(target.start),yd);
const ye=Y.createAbsolutePositionFromRelativePosition(Y.createRelativePositionFromJSON(target.end),yd);
assert.equal(yt.toString().slice(ys.index,ye.index),'TAR!GET');
assert.equal(yt.toDelta().filter(d=>d.attributes?.bold).map(d=>d.insert).join(''),'TAR!GET');
const results={automerge:{pass:true,quote:ad.text.slice(astart,aend),marks:A.marks(ad,['text'])},
  yjs_yrs:{pass:true,quote:yt.toString().slice(ys.index,ye.index),delta:yt.toDelta()},
  scope:'JavaScript -> native Rust mutation -> JavaScript reload; emoji, combining character, cursor ranges, formatting. Not full editor schema interoperability.'};
writeFileSync(new URL('interop-results.json',import.meta.url),JSON.stringify(results,null,2)+'\n');
console.log(JSON.stringify(results));
