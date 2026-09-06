import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { initSync } from '../src/generated/document/quarry_document';
import { DocumentModel } from '../src/features/editor/document-model';

initSync({ module: readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), '../src/generated/document/quarry_document_bg.wasm')) });
const paragraph = 'Review TARGET with Unicode 😀 and **formatted text**. ' + 'Detail '.repeat(12);
const markdown = Array.from({ length: 800 }, (_, index) => `${index}: ${paragraph}`).join('\n\n');
const started = performance.now();
const model = DocumentModel.fromMarkdown(markdown);
const imported = performance.now() - started;
const samples: number[] = [];
const id = model.view().blocks[400].block.id;
for (let n = 0; n < 30; n++) {
  const start = performance.now();
  model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point(id, n), text: 'x' }]));
  model.view(); model.save();
  samples.push(performance.now() - start);
}
const bytes = model.save(); const start = performance.now();
const restored = new DocumentModel(bytes); restored.view(); const reload = performance.now() - start;
if (JSON.stringify(restored.view()) !== JSON.stringify(model.view())) throw new Error('Benchmark reload changed native state');
samples.sort((a, b) => a - b);
console.log(JSON.stringify({ markdown_bytes: new TextEncoder().encode(markdown).length, blocks: model.view().blocks.length,
  native_bytes: bytes.length, import_ms: imported, edit_view_save_ms: { p50: samples[15], p95: samples[28], max: samples[29] }, reload_ms: reload }, null, 2));
restored.dispose(); model.dispose();
