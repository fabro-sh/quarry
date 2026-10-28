import { BaseListPlugin } from '@platejs/list';
import { BaseParagraphPlugin, createSlateEditor, NodeApi } from 'platejs';

test('editing and removing the final block do not query missing list siblings', () => {
  const editor = createSlateEditor({ plugins: [BaseParagraphPlugin, BaseListPlugin], value: [
    { type: 'p', children: [{ text: 'First' }] },
    { type: 'p', children: [{ text: 'Last' }] },
  ] });
  const node = editor.api.node;
  const missing: unknown[] = [];
  const spy = vi.spyOn(editor.api, 'node').mockImplementation(((at: unknown, ...args: unknown[]) => {
    if (Array.isArray(at) && !NodeApi.has(editor, at)) missing.push(at);
    return Reflect.apply(node, editor.api, [at, ...args]);
  }) as typeof node);
  try {
    editor.tf.setNodes({ quarryReviewKey: 'changed' }, { at: [1] });
    expect(editor.operations.some((operation) => operation.type === 'set_node')).toBe(true);
    editor.tf.removeNodes({ at: [1] });
    expect(editor.children).toEqual([{ type: 'p', children: [{ text: 'First' }] }]);
    expect(missing).toEqual([]);
  } finally { spy.mockRestore(); }
});
