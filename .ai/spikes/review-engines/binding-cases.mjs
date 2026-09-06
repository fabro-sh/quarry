import { wrapInList } from 'prosemirror-schema-list';
import { setBlockType } from 'prosemirror-commands';
import { engines, documentOf, paragraph, schema, textRange, selectText, snapshot } from './editor-engines.mjs';

const fixtures = [
  {name:'ASCII',paragraphs:['See docs and TARGET here.', 'Second paragraph.']},
  {name:'Unicode',paragraphs:['😀 e\u0301 See docs and TARGET here.', 'Second paragraph.']},
  {name:'Repeated text',paragraphs:['See docs and TARGET here.', 'See docs and TARGET elsewhere.']},
];
function comment(e, needle = 'TARGET', id = 'c1') {
  const r = textRange(e, needle); e.comment(id, r.from, r.to);
}
function typeInTarget(e) {
  const { from } = textRange(e, 'TARGET');
  e.view.dispatch(e.view.state.tr.insertText('!', from + 3));
}
function baseCheck(left, right, expectedQuote) {
  const quotes = [left, right].map(x => x.comments.find(c => c.id === 'c1')?.quote ?? null);
  const treeConverged=JSON.stringify(left.tree)===JSON.stringify(right.tree);
  return { pass: treeConverged && left.text === right.text && quotes.every(q => q === expectedQuote), quotes,
    textConverged: left.text === right.text,treeConverged };
}

const cases = initial => [
  {
    name: 'comment-only has no content effects', expected: 'TARGET',
    act([a,b]) { comment(a); comment(b, 'docs', 'c2'); },
    check(a,b) { return { ...baseCheck(a,b,'TARGET'), pass: baseCheck(a,b,'TARGET').pass && a.text === initial.join('') && a.comments.length === 2 }; },
  },
  {
    name: 'link formatting races typing and comment', expected: 'TAR!GET',
    act([a,b]) {
      const r = textRange(a,'docs');
      a.view.dispatch(a.view.state.tr.addMark(r.from,r.to,schema.marks.link.create({href:'https://example.test'})));
      comment(b); typeInTarget(b);
    },
  },
  {
    name: 'list wrapping races typing and comment', expected: 'TAR!GET',
    act([a,b]) {
      selectText(a,'TARGET');
      if (!wrapInList(schema.nodes.bullet_list)(a.view.state,tr=>a.view.dispatch(tr))) throw new Error('wrapInList unavailable');
      comment(b); typeInTarget(b);
    },
  },
  {
    name: 'heading conversion races typing and comment', expected: 'TAR!GET',
    act([a,b]) {
      selectText(a,'TARGET');
      setBlockType(schema.nodes.heading,{level:2})(a.view.state,tr=>a.view.dispatch(tr));
      comment(b); typeInTarget(b);
    },
  },
  {
    name: 'paragraph split races typing and comment', expected: 'TAR!GET',
    act([a,b]) {
      a.view.dispatch(a.view.state.tr.split(textRange(a,'TARGET').from));
      comment(b); typeInTarget(b);
    },
  },
  {
    name: 'delete and reinsert block move races typing and comment', expected: 'TAR!GET',
    act([a,b]) {
      const first = a.view.state.doc.firstChild;
      const tr = a.view.state.tr.delete(0,first.nodeSize);
      tr.insert(tr.doc.content.size,first);
      a.view.dispatch(tr); comment(b); typeInTarget(b);
    },
  },
  {
    name: 'partial target deletion preserves surviving text', expected: 'GET',
    pre(pair,Engine) { comment(pair[0]); Engine.sync(pair); },
    act([a]) { const r=textRange(a,'TARGET'); a.view.dispatch(a.view.state.tr.delete(r.from,r.from+3)); },
  },
  {
    name: 'whole target deletion retains thread with empty target', expected: '',
    pre(pair,Engine) { comment(pair[0]); Engine.sync(pair); },
    act([a]) { const r=textRange(a,'TARGET'); a.view.dispatch(a.view.state.tr.delete(r.from,r.to)); },
  },
  {
    name: 'text insertion preceding newly created comment', expected: 'INSERTED',
    act([a,b]) {
      const r=textRange(b,'TARGET'); b.view.dispatch(b.view.state.tr.insertText('INSERTED ',r.from));
      comment(b,'INSERTED');
      a.view.dispatch(a.view.state.tr.insertText('Earlier ',1));
    },
  },
  {
    name: 'save and restore existing cursor targets', expected: 'TARGET',
    act([a]) { comment(a); },
    restore: true,
  },
];

export async function runBindingCases(onResult = () => {}) {
  const results=[];
  for (const fixture of fixtures) for (const Engine of engines) for (const test of cases(fixture.paragraphs)) for (const reverse of [false,true]) {
    let pair=[];
    const start=performance.now();
    let result;
    try {
      pair=Engine.pair(documentOf(fixture.paragraphs));
      test.pre?.(pair,Engine);
      test.act(pair);
      Engine.sync(pair,reverse);
      // Repeated delivery also tests duplicate-safe transport handling.
      Engine.sync(pair,reverse);
      if (test.restore) {
        const restored=Engine.restore(pair[0].save());
        pair[0].dispose(); pair[0]=restored;
      }
      const left=snapshot(pair[0]),right=snapshot(pair[1]);
      const check=test.check?.(left,right) ?? baseCheck(left,right,test.expected);
      if(test.name.includes('races typing')) {
        const moved=test.name.startsWith('delete and reinsert');
        const expectedText=moved?fixture.paragraphs[1]+fixture.paragraphs[0].replace('TARGET','TAR!GET'):
          fixture.paragraphs.join('').replace('TARGET','TAR!GET');
        check.expectedText=expectedText;
        check.editPreserved=left.text===expectedText && right.text===expectedText;
        check.pass &&= check.editPreserved;
        if(test.name.startsWith('list wrapping'))check.structurePreserved=left.tree.content[0].type==='bullet_list';
        if(test.name.startsWith('heading'))check.structurePreserved=left.tree.content[0].type==='heading'&&left.tree.content[0].attrs.level===2;
        if(test.name.startsWith('paragraph split'))check.structurePreserved=left.tree.content.length===3;
        if(test.name.startsWith('link'))check.structurePreserved=JSON.stringify(left.tree).includes('https://example.test');
        if(check.structurePreserved===false)check.pass=false;
      }
      result={fixture:fixture.name,engine:Engine.name,case:test.name,delivery:reverse?'reverse':'forward',expectedQuote:test.expected,
        ...check,left,right,milliseconds:performance.now()-start};
    } catch(error) {
      result={fixture:fixture.name,engine:Engine.name,case:test.name,delivery:reverse?'reverse':'forward',pass:false,
        error:error.stack ?? String(error),milliseconds:performance.now()-start};
    } finally { for (const e of pair) { try { e.dispose(); } catch {} } }
    results.push(result); onResult(result);
    await new Promise(resolve=>setTimeout(resolve,0));
  }
  return results;
}
