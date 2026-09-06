// Real upstream editor bindings. The handle below only controls delivery;
// it does not translate or repair editor operations.
import * as A from '@automerge/automerge';
import * as Y from 'yjs';
import { init, basicSchemaAdapter, pmNodeToSpans } from '@automerge/prosemirror';
import { pmRangeToAmRange, amSpliceIdxToPmIdx } from '@automerge/prosemirror/dist/traversal.js';
import { EditorState, TextSelection } from 'prosemirror-state';
import { EditorView } from 'prosemirror-view';
import { Step } from 'prosemirror-transform';
import { collab, getVersion, sendableSteps, receiveTransaction } from 'prosemirror-collab';
import {
  ySyncPlugin, ySyncPluginKey, prosemirrorToYDoc, initProseMirrorDoc,
  absolutePositionToRelativePosition, relativePositionToAbsolutePosition,
} from 'y-prosemirror';

export { A, Y, basicSchemaAdapter, TextSelection };
export const schema = basicSchemaAdapter.schema;
export const paragraph = text => schema.nodes.paragraph.create(null, text ? schema.text(text) : null);
export const documentOf = paragraphs => schema.nodes.doc.create(null, paragraphs.map(paragraph));

function editor(doc, plugins, onTransaction = () => {}) {
  const mount = document.createElement('div');
  mount.className = 'editor';
  document.body.append(mount);
  const view = new EditorView(mount, {
    state: EditorState.create({ schema, doc, plugins }),
    dispatchTransaction(tr) {
      onTransaction(tr);
      this.updateState(this.state.apply(tr));
    },
  });
  return view;
}

class ControlledHandle {
  constructor(doc) { this.current = doc; this.listeners = new Set(); }
  doc() { return this.current; }
  on(_, fn) { this.listeners.add(fn); }
  off(_, fn) { this.listeners.delete(fn); }
  change(fn) {
    let notification;
    this.current = A.change(this.current, {
      patchCallback: (patches, patchInfo) => { notification = { patches, patchInfo }; },
    }, fn);
    this.emit(notification);
  }
  receive(changes) {
    let notification;
    [this.current] = A.applyChanges(this.current, changes, {
      patchCallback: (patches, patchInfo) => { notification = { patches, patchInfo }; },
    });
    this.emit(notification);
  }
  emit(notification) {
    if (notification) for (const fn of this.listeners) fn({ ...notification, handle: this, doc: this.current });
  }
}

export class AutomergeEditor {
  static name = 'Automerge + ProseMirror';
  static pair(pmDoc) {
    let doc = A.from({ text: '', threads: {} }, { actor: 'a1' });
    doc = A.change(doc, d => A.updateSpans(d, ['text'], pmNodeToSpans(basicSchemaAdapter, pmDoc)));
    return [new this(A.clone(doc, { actor: 'b1' })), new this(A.clone(doc, { actor: 'c1' }))];
  }
  constructor(doc) {
    this.handle = new ControlledHandle(doc);
    const setup = init(this.handle, ['text']);
    this.view = editor(setup.pmDoc, [setup.plugin]);
  }
  comment(id, from, to) {
    const doc = this.handle.doc();
    const range = pmRangeToAmRange(basicSchemaAdapter, A.spans(doc, ['text']), { from, to });
    if (!range) throw new Error('Cannot map selection to Automerge');
    const target = {
      start: A.getCursor(doc, ['text'], range.start, 'after'),
      end: A.getCursor(doc, ['text'], range.end, 'before'),
      quote: this.view.state.doc.textBetween(from, to, '\n'),
    };
    this.handle.change(d => { d.threads[id] = target; });
  }
  comments() {
    const doc = this.handle.doc();
    const spans = A.spans(doc, ['text']);
    return Object.entries(doc.threads).map(([id, t]) => {
      try {
        const start = A.getCursorPosition(doc, ['text'], t.start);
        const end = A.getCursorPosition(doc, ['text'], t.end);
        const from = amSpliceIdxToPmIdx(basicSchemaAdapter, spans, start);
        const to = amSpliceIdxToPmIdx(basicSchemaAdapter, spans, end);
        return { id, original: t.quote, from, to,
          quote: from != null && to != null && from <= to ? this.view.state.doc.textBetween(from, to, '\n') : null };
      } catch (error) { return { id, original: t.quote, quote: null, error: error.message }; }
    });
  }
  static sync(pair, reverse = false) {
    const changes = pair.map(e => A.getAllChanges(e.handle.doc()));
    pair[0].handle.receive(reverse ? [...changes[1]].reverse() : changes[1]);
    pair[1].handle.receive(reverse ? [...changes[0]].reverse() : changes[0]);
  }
  save() { return A.save(this.handle.doc()); }
  static restore(bytes) { return new this(A.load(bytes)); }
  dispose() { this.view.destroy(); this.view.dom.parentElement?.remove(); }
}

export class YjsEditor {
  static name = 'Yjs + ProseMirror';
  static pair(pmDoc) {
    const source = prosemirrorToYDoc(pmDoc);
    const bytes = Y.encodeStateAsUpdate(source);
    source.destroy();
    return [this.restore(bytes, 101), this.restore(bytes, 102)];
  }
  constructor(doc) {
    this.doc = doc;
    this.root = doc.getXmlFragment('prosemirror');
    const setup = initProseMirrorDoc(this.root, schema);
    this.view = editor(setup.doc, [ySyncPlugin(this.root, { mapping: setup.mapping })]);
  }
  mapping() { return ySyncPluginKey.getState(this.view.state).binding.mapping; }
  comment(id, from, to) {
    const target = {
      start: Y.relativePositionToJSON(absolutePositionToRelativePosition(from, this.root, this.mapping())),
      end: Y.relativePositionToJSON(absolutePositionToRelativePosition(to, this.root, this.mapping())),
      quote: this.view.state.doc.textBetween(from, to, '\n'),
    };
    this.doc.getMap('threads').set(id, target);
  }
  comments() {
    return [...this.doc.getMap('threads')].map(([id, t]) => {
      const from = relativePositionToAbsolutePosition(this.doc, this.root, Y.createRelativePositionFromJSON(t.start), this.mapping());
      const to = relativePositionToAbsolutePosition(this.doc, this.root, Y.createRelativePositionFromJSON(t.end), this.mapping());
      return { id, original: t.quote, from, to,
        quote: from != null && to != null && from <= to ? this.view.state.doc.textBetween(from, to, '\n') : null };
    });
  }
  static sync(pair, reverse = false) {
    const updates = pair.map(e => Y.encodeStateAsUpdate(e.doc));
    for(const i of reverse?[1,0]:[0,1])Y.applyUpdate(pair[i].doc,updates[1-i]);
  }
  save() { return Y.encodeStateAsUpdate(this.doc); }
  static restore(bytes, clientID) { const doc = new Y.Doc(); Y.applyUpdate(doc, bytes); if (clientID) doc.clientID = clientID; return new this(doc); }
  dispose() { this.view.destroy(); this.view.dom.parentElement?.remove(); this.doc.destroy(); }
}

// Central authority: no custom rebasing algorithm. ProseMirror's collab
// plugin rebases pending browser steps. Quarry must supply the sidecar
// mapping and ordered review-command logic shown explicitly here.
function mapTarget(target, mapping) {
  const start = mapping.mapResult(target.from, 1);
  const end = mapping.mapResult(target.to, -1);
  return { ...target, from: Math.min(start.pos, end.pos), to: Math.max(start.pos, end.pos),
    deleted: target.deleted || (start.deletedAcross && end.deletedAcross) || start.pos >= end.pos };
}

export class CentralEditor {
  static name = 'Central ProseMirror';
  static pair(pmDoc) {
    const authority = { doc: pmDoc, steps: [], clients: [], threads: new Map(), commits: 0 };
    return [new this(authority, 'left'), new this(authority, 'right')];
  }
  constructor(authority, id) {
    this.authority = authority;
    this.id = id;
    this.pending = new Map();
    this.view = editor(authority.doc, [collab({ version: authority.steps.length, clientID: id })], tr => {
      for (const [id, target] of this.pending) this.pending.set(id, mapTarget(target, tr.mapping));
    });
  }
  comment(id, from, to) {
    this.pending.set(id, { from, to, quote: this.view.state.doc.textBetween(from, to, '\n'), deleted: false });
  }
  comments() {
    const pendingSteps = sendableSteps(this.view.state)?.steps ?? [];
    const items = new Map(this.authority.threads);
    for (const [id, target] of this.pending) items.set(id, target);
    return [...items].map(([id, original]) => {
      let target = original;
      if (!this.pending.has(id)) for (const step of pendingSteps) target = mapTarget(target, step.getMap());
      return { id, original: target.quote, from: target.from, to: target.to, deleted: target.deleted,
        quote: target.from <= target.to ? this.view.state.doc.textBetween(target.from, target.to, '\n') : null };
    });
  }
  pull() {
    const offset = getVersion(this.view.state);
    if (offset < this.authority.steps.length) {
      this.view.dispatch(receiveTransaction(this.view.state,
        this.authority.steps.slice(offset), this.authority.clients.slice(offset)));
    }
  }
  submit() {
    const a = this.authority;
    const send = sendableSteps(this.view.state);
    if ((send?.version ?? getVersion(this.view.state)) !== a.steps.length) return false;
    if (send) for (const step of send.steps) {
      const result = step.apply(a.doc);
      if (result.failed) throw new Error(result.failed);
      for (const [id, target] of a.threads) a.threads.set(id, mapTarget(target, step.getMap()));
      a.doc = result.doc;
      a.steps.push(step); a.clients.push(this.id);
    }
    for (const [id, target] of this.pending) a.threads.set(id, target);
    this.pending.clear();
    a.commits += 1;
    this.pull();
    return true;
  }
  static sync(pair, reverse = false) {
    const order = reverse ? [...pair].reverse() : pair;
    for (const e of order) {
      if (!e.submit()) { e.pull(); if (!e.submit()) throw new Error('Unexpected second stale submission'); }
    }
    for (const e of pair) e.pull();
  }
  save() {
    this.submit();
    return new TextEncoder().encode(JSON.stringify({ doc: this.authority.doc.toJSON(),
      threads: [...this.authority.threads],steps:this.authority.steps.map(s=>s.toJSON()),clients:this.authority.clients }));
  }
  static restore(bytes) {
    const stored = JSON.parse(new TextDecoder().decode(bytes));
    return new this({ doc: schema.nodeFromJSON(stored.doc), threads: new Map(stored.threads),
      steps:(stored.steps??[]).map(s=>Step.fromJSON(schema,s)),clients:stored.clients??[],commits:0 }, 'restored');
  }
  dispose() { this.view.destroy(); this.view.dom.parentElement?.remove(); }
}

export const engines = [AutomergeEditor, YjsEditor, CentralEditor];

export function textRange(engine, needle) {
  let found;
  engine.view.state.doc.descendants((node, pos) => {
    if (found || !node.isText) return;
    const offset = node.text.indexOf(needle);
    if (offset >= 0) found = { from: pos + offset, to: pos + offset + needle.length };
  });
  if (!found) throw new Error(`Text not found: ${needle}; document: ${engine.view.state.doc.textContent}`);
  return found;
}

export function selectText(engine, needle) {
  const { from, to } = textRange(engine, needle);
  engine.view.dispatch(engine.view.state.tr.setSelection(TextSelection.create(engine.view.state.doc, from, to)));
  return { from, to };
}

export function snapshot(engine) {
  return { text: engine.view.state.doc.textContent, tree: engine.view.state.doc.toJSON(),
    comments: engine.comments() };
}
