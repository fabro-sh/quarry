import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';

for (const mode of ['development', 'production']) for (const fallback of [false, true]) test(`React ${mode} ${fallback ? 'fallback' : 'native Range'} restores exact selection without reading past its endpoints`, () => {
  const output = execFileSync(process.execPath, [resolve('src/features/editor/__fixtures__/react-selection.mjs')], {
    env: { ...process.env, NODE_ENV: mode, REACT_SELECTION_FALLBACK: fallback ? '1' : '0' }, encoding: 'utf8',
  });
  expect(JSON.parse(output)).toEqual({ mode, fallback, cases: 6 });
});
