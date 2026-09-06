import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';

const { window } = new JSDOM('<!doctype html><html><body><div id="root"></div></body></html>', { url: 'http://localhost' });
for (const key of ['window', 'document', 'navigator', 'Node', 'HTMLElement']) Object.defineProperty(globalThis, key, { value: key === 'window' ? window : window[key], configurable: true });
const React = (await import('react')).default;
const { createRoot } = await import('react-dom/client');
const { flushSync } = await import('react-dom');
const h = React.createElement;

class Fixture extends React.Component {
  getSnapshotBeforeUpdate() {
    document.querySelector('button').focus();
    window.getSelection().removeAllRanges();
    return null;
  }
  componentDidUpdate() {}
  render() {
    return h('section', { 'data-tick': this.props.tick }, h('button', {}, 'Focus elsewhere'),
      h('div', { contentEditable: true, suppressContentEditableWarning: true, tabIndex: 0 },
        ...Array.from({ length: 1000 }, (_, index) => h('span', { key: index, 'data-i': index },
          index === 1 ? h('b', {}, 'second ') : index === 0 ? '😀 first ' : `text ${index} `))));
  }
}
const root = createRoot(document.getElementById('root'));
flushSync(() => root.render(h(Fixture, { tick: 0 })));
const editable = document.querySelector('[contenteditable]');
// jsdom does not expose the browser's reflected contentEditable property.
Object.defineProperty(editable, 'contentEditable', { value: 'true', configurable: true });
// Browsers retain a selection inside an element when focusing it. jsdom resets
// it, including the range React has just restored before its focus() call.
const focusEditable = editable.focus.bind(editable);
editable.focus = () => {
  const selection = window.getSelection();
  const saved = editable.contains(selection.anchorNode) && editable.contains(selection.focusNode)
    ? [selection.anchorNode, selection.anchorOffset, selection.focusNode, selection.focusOffset] : null;
  focusEditable();
  if (saved) selection.setBaseAndExtent(...saved);
};
const text = (index) => document.querySelector(`[data-i="${index}"]`).firstChild;
const cases = [
  [[text(0), 2], [text(0), 2]],
  [[text(0), 3], [text(1).firstChild, 5]],
  [[text(1).firstChild, 5], [text(0), 3]],
  [[editable, 0], [editable, 2]],
  [[editable, 2], [editable, 0]],
  [[text(999), 2], [text(999), 5]],
];
const logical = (node, offset) => {
  const range = document.createRange(); range.setStart(editable, 0); range.setEnd(node, offset); return range.toString().length;
};
const tail = text(999), nodeValue = Object.getOwnPropertyDescriptor(window.Node.prototype, 'nodeValue');
let tailReads = 0;
Object.defineProperty(tail, 'nodeValue', { get() { tailReads++; return nodeValue.get.call(this); }, set(value) { nodeValue.set.call(this, value); }, configurable: true });
for (const [index, [anchor, focus]] of cases.entries()) {
  editable.focus();
  const selection = window.getSelection(); selection.setBaseAndExtent(...anchor, ...focus);
  const expected = [logical(...anchor), logical(...focus)];
  tailReads = 0;
  flushSync(() => root.render(h(Fixture, { tick: index + 1 })));
  assert.equal(document.activeElement, editable, 'React restores editor focus after the snapshot callback changes it');
  assert.deepEqual([logical(selection.anchorNode, selection.anchorOffset), logical(selection.focusNode, selection.focusOffset)], expected, `selection case ${index}`);
  if (index < cases.length - 1) assert.equal(tailReads, 0, 'selection capture must stop before unrelated distant text');
}
flushSync(() => root.unmount());
window.close();
console.log(JSON.stringify({ mode: process.env.NODE_ENV, cases: cases.length }));
