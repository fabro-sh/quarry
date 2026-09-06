// The binding and Slate versions already installed by Quarry. No Plate UI or
// server row reconciliation. This tests the smaller "keep Slate, persist Yjs" alternative.
import {createRequire} from 'node:module';
import {writeFileSync} from 'node:fs';
const require=createRequire(new URL('../../../ui/package.json',import.meta.url));
const Y=require('yjs');
const {createEditor,Editor,Node,Text,Transforms}=require('slate');
const {withYjs,YjsEditor,slateNodesToInsertDelta,slateRangeToRelativeRange,relativeRangeToSlateRange}=require('@slate-yjs/core');
function open(doc){
  const root=doc.get('content',Y.XmlText),editor=withYjs(createEditor(),root,{autoConnect:false});
  editor.isInline=n=>n.type==='a';YjsEditor.connect(editor);return {doc,root,editor};
}
function pair(paragraphs){
  const seed=new Y.Doc();seed.get('content',Y.XmlText).applyDelta(slateNodesToInsertDelta(paragraphs.map((text,i)=>({id:'b'+i,type:'p',children:[{text}]}))));
  return [101,102].map(id=>{const doc=new Y.Doc();Y.applyUpdate(doc,Y.encodeStateAsUpdate(seed));doc.clientID=id;return open(doc);});
}
function range(e,needle){for(const [node,path]of Node.texts(e.editor)){const offset=node.text.indexOf(needle);if(offset>=0)return {anchor:{path,offset},focus:{path,offset:offset+needle.length}};}throw Error('Missing '+needle);}
function flush(e){YjsEditor.flushLocalChanges(e.editor);}
function comment(e,needle='TARGET',storage='relative'){
  flush(e);const r=range(e,needle);
  if(storage==='stored'){YjsEditor.storePosition(e.editor,'c1:start',r.anchor);YjsEditor.storePosition(e.editor,'c1:end',r.focus);e.doc.getMap('threads').set('c1',{storage});}
  else {const relative=slateRangeToRelativeRange(e.root,e.editor,r);e.doc.getMap('threads').set('c1',{anchor:Y.relativePositionToJSON(relative.anchor),focus:Y.relativePositionToJSON(relative.focus)});}
}
function read(e){
  const t=e.doc.getMap('threads').get('c1');let r=null;
  if(t?.storage==='stored'){const anchor=YjsEditor.position(e.editor,'c1:start'),focus=YjsEditor.position(e.editor,'c1:end');if(anchor&&focus)r={anchor,focus};}
  else if(t)r=relativeRangeToSlateRange(e.root,e.editor,{anchor:Y.createRelativePositionFromJSON(t.anchor),focus:Y.createRelativePositionFromJSON(t.focus)});
  return {text:Node.string(e.editor),tree:e.editor.children,quote:r?Editor.string(e.editor,r):null};
}
function sync(p,reverse){p.forEach(flush);const updates=p.map(e=>Y.encodeStateAsUpdate(e.doc));for(const i of reverse?[1,0]:[0,1])Y.applyUpdate(p[i].doc,updates[1-i]);}
function typing(e){const r=range(e,'TARGET');Transforms.insertText(e.editor,'!',{at:{path:r.anchor.path,offset:r.anchor.offset+3}});}
const scenarios=[
  {name:'comment-only has no content effects',op(){},expected:'TARGET',type:false},
  {name:'link formatting races typing and comment',op(e){Transforms.wrapNodes(e.editor,{type:'a',url:'https://example.test',children:[]},{at:range(e,'docs'),split:true});}},
  {name:'Quarry-style list attributes race typing and comment',op(e){Transforms.setNodes(e.editor,{listStyleType:'disc',indent:1},{at:[0]});}},
  {name:'heading conversion races typing and comment',op(e){Transforms.setNodes(e.editor,{type:'h2'},{at:[0]});}},
  {name:'paragraph split races typing and comment',op(e){Transforms.splitNodes(e.editor,{at:range(e,'TARGET').anchor,always:true});}},
  {name:'Slate move_node races typing and comment',op(e){Transforms.moveNodes(e.editor,{at:[0],to:[1]});},move:true},
  {name:'partial target deletion preserves surviving text',existing:true,type:false,expected:'GET',op(e){const r=range(e,'TARGET');r.focus.offset=r.anchor.offset+3;Transforms.delete(e.editor,{at:r});}},
  {name:'whole target deletion retains thread with empty target',existing:true,type:false,expected:'',op(e){Transforms.delete(e.editor,{at:range(e,'TARGET')});}},
  {name:'save and restore existing cursor targets',type:false,expected:'TARGET',restore:true,op(){}},
];
const results=[];
for(const paragraphs of [['See docs and TARGET here.','Second paragraph.'],['😀 e\u0301 See docs and TARGET here.','Second paragraph.'],['See docs and TARGET here.','See docs and TARGET elsewhere.']])
for(const s of scenarios)for(const reverse of [false,true]){
  const p=pair(paragraphs);let result;
  try{
    if(s.existing){comment(p[1]);sync(p,reverse);}s.op(p[0]);
    if(!s.existing)comment(p[1]);if(s.type!==false)typing(p[1]);sync(p,reverse);sync(p,reverse);
    if(s.restore){const doc=new Y.Doc();Y.applyUpdate(doc,Y.encodeStateAsUpdate(p[0].doc));YjsEditor.disconnect(p[0].editor);p[0].doc.destroy();p[0]=open(doc);}
    const left=read(p[0]),right=read(p[1]),expected=s.expected??'TAR!GET';
    const expectedText=s.move?paragraphs[1]+paragraphs[0].replace('TARGET','TAR!GET'):
      paragraphs.join('').replace('TARGET',expected);
    const treeConverged=JSON.stringify(left.tree)===JSON.stringify(right.tree);
    result={engine:'Yjs + Slate core',case:s.name,fixture:paragraphs[0].startsWith('😀')?'Unicode':paragraphs[1].startsWith('See')?'Repeated text':'ASCII',delivery:reverse?'reverse':'forward',
      pass:treeConverged&&left.text===expectedText&&right.text===expectedText&&left.quote===expected&&right.quote===expected,
      treeConverged,expectedQuote:expected,expectedText,left,right};
  }catch(error){result={engine:'Yjs + Slate core',case:s.name,pass:false,error:error.stack};}
  for(const e of p){YjsEditor.disconnect(e.editor);e.doc.destroy();}
  results.push(result);
}
// Upstream offers migration for stored positions. Prove its visibility limit.
for(const known of [true,false]){
  const p=pair(['See docs and TARGET here.','Second paragraph.']);
  comment(p[1],'TARGET','stored');if(known)sync(p,false);
  Transforms.moveNodes(p[0].editor,{at:[0],to:[1]});sync(p,false);
  const left=read(p[0]),right=read(p[1]);
  results.push({engine:'Yjs + Slate stored positions',case:known?'move sees existing target':'move cannot see in-flight target',pass:left.quote==='TARGET'&&right.quote==='TARGET',left,right});
  p.forEach(e=>{YjsEditor.disconnect(e.editor);e.doc.destroy();});
}
writeFileSync(new URL('slate-results.json',import.meta.url),JSON.stringify({runtime:process.version,versions:{slate:require('slate/package.json').version,yjs:require('yjs/package.json').version,slateYjs:'1.0.2'},
  environment:'Quarry installed Slate and slate-yjs core; no React/Plate UI or Rust reconciliation',results},null,2)+'\n');
for(const r of results)console.log(`${r.pass?'PASS':'MISS'} ${r.case} ${r.error??JSON.stringify([r.left.quote,r.right.quote])}`);
console.log(JSON.stringify({cases:results.length,passes:results.filter(r=>r.pass).length,errors:results.filter(r=>r.error).length}));
if(results.some(r=>r.error))process.exitCode=1;
