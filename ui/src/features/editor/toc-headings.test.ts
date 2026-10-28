import { createSlateEditor } from 'platejs';
import { queryHeadings } from './toc-headings';

test('headings retain identity across paragraph edits and follow heading edits, moves and removal', () => {
  const editor = createSlateEditor({ value: [
    { type: 'h1', id: 'first', children: [{ text: 'First' }] },
    { type: 'p', id: 'body', children: [{ text: 'Body' }] },
    { type: 'blockquote', id: 'quote', children: [{ type: 'h3', id: 'nested', children: [{ text: 'Nested' }] }] },
  ] });
  const initial = queryHeadings(editor);
  expect(initial).toEqual([
    { id: 'first', type: 'h1', depth: 1, title: 'First', path: [0] },
    { id: 'nested', type: 'h3', depth: 3, title: 'Nested', path: [2, 0] },
  ]);
  editor.tf.insertText('New ', { at: { path: [1, 0], offset: 0 } });
  expect(queryHeadings(editor)).toBe(initial);
  editor.tf.insertText('New ', { at: { path: [0, 0], offset: 0 } });
  expect(queryHeadings(editor)[0].title).toBe('New First');
  editor.tf.moveNodes({ at: [0], to: [2] });
  expect(queryHeadings(editor).map(({ id, path }) => ({ id, path }))).toEqual([
    { id: 'nested', path: [1, 0] }, { id: 'first', path: [2] },
  ]);
  editor.tf.setNodes({ type: 'h2' }, { at: [2] });
  expect(queryHeadings(editor)[1]).toMatchObject({ type: 'h2', depth: 2 });
  editor.tf.removeNodes({ at: [1] });
  expect(queryHeadings(editor)).toEqual([{ id: 'first', type: 'h2', depth: 2, title: 'New First', path: [1] }]);
});
