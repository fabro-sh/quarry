import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';

for (const mode of ['development', 'production']) test(`React ${mode} restores exact selection without reading past its endpoints`, () => {
  const output = execFileSync(process.execPath, [resolve('src/features/editor/__fixtures__/react-selection.mjs')], {
    env: { ...process.env, NODE_ENV: mode }, encoding: 'utf8',
  });
  expect(JSON.parse(output)).toEqual({ mode, cases: 6 });
});
