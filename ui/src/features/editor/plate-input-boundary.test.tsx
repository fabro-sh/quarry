import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { act, cleanup, render, screen } from '@testing-library/react';
import { initSync } from '../../generated/document/quarry_document';
import { NativeReviewContext } from '../review/native-review-context';
import { DocumentModel } from './document-model';
import { PlateMarkdownEditor } from './PlateMarkdownEditor';
import type { PlateDocumentAdapter } from './plate-document-adapter';

// jsdom does not expose this browser capability. Enable Slate's native event
// listener; each event below supplies its real DOM range.
vi.hoisted(() => {
  Object.defineProperty(InputEvent.prototype, 'getTargetRanges', { configurable: true, value: () => [] });
  Object.defineProperty(HTMLElement.prototype, 'isContentEditable', { configurable: true,
    get() { return this.closest('[contenteditable]')?.getAttribute('contenteditable') === 'true'; } });
});
beforeAll(() => initSync({ module: readFileSync(resolve('src/generated/document/quarry_document_bg.wasm')) }));
afterEach(cleanup);

test('beforeinput captures the browser selection and never restores typing to the previous block', async () => {
  const model = DocumentModel.fromMarkdown('First\n\nSecond');
  const errors: unknown[] = [];
  let adapter!: PlateDocumentAdapter;
  const review = { document: model.view(), author: 'Reviewer', readOnly: false, activeId: null, hoverId: null,
    setActiveId: () => {}, setHoverId: () => {}, command: () => true, focusTarget: () => {} };
  try {
    render(<NativeReviewContext.Provider value={review}>
      <PlateMarkdownEditor model={model} review={review} mode="editing" peers={[]}
        options={{ changed: () => {}, error: (error) => errors.push(error) }}
        onReady={(value) => { adapter = value; }} onComment={() => {}} onCompositionChange={() => {}} />
    </NativeReviewContext.Provider>);
    const editor = screen.getByRole('textbox', { name: 'Plate markdown editor' });
    expect(adapter.editor.api.hasEditableTarget(editor)).toBe(true);
    await act(async () => { adapter.editor.tf.select({ path: [0, 0], offset: 0 }); editor.focus(); });
    const second = editor.querySelectorAll('[data-slate-string]')[1].firstChild!;
    await act(async () => {
      // A browser selection can change before the throttled selectionchange
      // reaches Slate. Deliver beforeinput in that exact interval.
      const range = document.createRange(); range.setStart(second, 0); range.collapse(true);
      const selection = window.getSelection()!; selection.removeAllRanges(); selection.addRange(range);
      expect(adapter.editor.selection?.anchor.path[0]).toBe(0);
      const event = new InputEvent('beforeinput', { inputType: 'insertText', data: '[', bubbles: true, cancelable: true });
      Object.defineProperty(event, 'getTargetRanges', { value: () => [range] });
      editor.dispatchEvent(event);
      adapter.flush();
    });
    expect(model.view().blocks.map((view) => view.text)).toEqual(['First', '[Second']);
    expect(adapter.editor.selection?.anchor.path[0]).toBe(1);
    await act(async () => { adapter.editor.tf.insertText('next'); adapter.flush(); });
    expect(model.view().blocks.map((view) => view.text)).toEqual(['First', '[nextSecond']);
    expect(errors).toEqual([]);
  } finally { cleanup(); model.dispose(); }
});
