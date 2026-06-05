import { spawnSync } from 'node:child_process';
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { slateNodesToInsertDelta, yTextToSlateElement } from '@slate-yjs/core';
import type { Value } from 'platejs';
import * as Y from 'yjs';

import { markdownToPlateValue } from '../src/features/editor/markdown-codec';
import { markdownToReview } from '../src/features/review/rfm-codec';

interface FixtureCase {
  name: string;
  markdown: string;
  supported: boolean;
  reason?: string;
  /** Which UI codec is the oracle. Defaults to the editor/collab codec. */
  codec?: 'editor' | 'review';
}

const CASES: FixtureCase[] = [
  {
    name: 'core-inline',
    supported: true,
    markdown:
      '# Heading\n\nParagraph **bold** *italic* <u>under</u> ~~strike~~ `code` [link](guide.md) [[Doc|Label]].\n',
  },
  {
    name: 'nested-list',
    supported: true,
    markdown: '- one\n  - nested\n- two\n',
  },
  {
    name: 'ordered-list',
    supported: true,
    markdown: '3. three\n4. four\n',
  },
  {
    name: 'task-list',
    supported: true,
    markdown: '- [ ] Todo\n- [x] Done\n',
  },
  {
    name: 'blocks',
    supported: true,
    markdown:
      '![alt](assets/x.png)\n\n```mermaid\ngraph TD; A-->B;\n```\n\n```rust\nfn main() {}\n```\n\n---\n',
  },
  {
    name: 'table',
    supported: true,
    markdown: '| L | C | R |\n| :-- | :-: | --: |\n| **Ana** | `dev` | 3 |\n',
  },
  {
    name: 'utf16',
    supported: true,
    markdown: `# \u{1f600} UTF16\n\nA \u{1f44d} **B**\n`,
  },
  {
    name: 'zero-width-placeholder',
    supported: true,
    markdown: '\u{200b}\n\n',
  },
  {
    name: 'critic-markup',
    supported: false,
    reason: 'CriticMarkup belongs to the review codec and must fall back.',
    markdown: 'See {==here==}{#c1}.\n',
  },
  {
    name: 'review-heading-comment',
    supported: true,
    codec: 'review',
    markdown:
      '# {==Local Light==}{>>note<<}{#c1}\n\nBody line.\n\n---\ncomments:\n  c1:\n    by: Claude\n    at: 2026-06-05T12:15:45.171Z\n',
  },
  {
    name: 'review-comment',
    supported: true,
    codec: 'review',
    markdown: 'See {==here==}{>>note<<}{#c1}.\n',
  },
  {
    name: 'review-comment-only',
    supported: true,
    codec: 'review',
    markdown: 'Note {>>aside<<}{#c2} done\n',
  },
  {
    name: 'review-suggestion',
    supported: true,
    codec: 'review',
    markdown:
      'Add {++word++}{#s1} and drop {--gone--}{#s2}.\n\n---\nsuggestions:\n  s1:\n    by: ai:codex\n    at: 2026-06-05T02:41:00.480Z\n  s2:\n    by: ai:claude\n    at: 2026-06-05T02:41:00.480Z\n',
  },
  {
    name: 'review-substitution',
    supported: true,
    codec: 'review',
    markdown:
      'Use {~~old~>new~~}{#s3} please\n\n---\nsuggestions:\n  s3:\n    by: ai:claude\n    at: 2026-06-05T02:41:00.480Z\n',
  },
  {
    name: 'footnote',
    supported: false,
    reason: 'Footnotes are outside the injected Plate surface.',
    markdown: 'A note [^1].\n\n[^1]: Footnote body.\n',
  },
  {
    name: 'definition-list',
    supported: false,
    reason: 'Definition lists are outside the injected Plate surface.',
    markdown: 'Term\n: Definition\n',
  },
];

const scriptDir = dirname(fileURLToPath(import.meta.url));
const uiRoot = resolve(scriptDir, '..');
const repoRoot = resolve(uiRoot, '..');
const fixtureRoot = resolve(repoRoot, 'fixtures/slate-yjs-compat');
const casesDir = resolve(fixtureRoot, 'cases');

await rm(casesDir, { force: true, recursive: true });
await mkdir(casesDir, { recursive: true });

for (const fixture of CASES) {
  await writeFile(
    resolve(casesDir, `${fixture.name}.json`),
    `${JSON.stringify(buildFixture(fixture), null, 2)}\n`
  );
}

const packageJson = JSON.parse(await readFile(resolve(uiRoot, 'package.json'), 'utf8')) as {
  dependencies: Record<string, string>;
};
const manifest = {
  schemaVersion: 1,
  generatedBy: 'ui/scripts/gen-slate-yjs-fixtures.ts',
  gitSha: commandOutput('git', ['rev-parse', '--short', 'HEAD']) || 'unknown',
  versions: {
    '@platejs/markdown': packageJson.dependencies['@platejs/markdown'],
    '@slate-yjs/core': packageJson.dependencies['@slate-yjs/core'],
    platejs: packageJson.dependencies.platejs,
    yjs: packageJson.dependencies.yjs,
  },
  cases: CASES.map((fixture) => fixture.name),
};
await writeFile(resolve(fixtureRoot, 'manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`);

function buildFixture(fixture: FixtureCase) {
  if (!fixture.supported) return fixture;

  const canonicalPlate =
    fixture.codec === 'review'
      ? markdownToReview(fixture.markdown).value
      : markdownToPlateValue(fixture.markdown);
  const doc = new Y.Doc();
  const root = doc.get('content', Y.XmlText);
  root.applyDelta(slateNodesToInsertDelta(canonicalPlate as Value));
  const observableState = yTextToSlateElement(root).children;

  return {
    ...fixture,
    canonicalPlate,
    observableState,
  };
}

function commandOutput(command: string, args: string[]) {
  const result = spawnSync(command, args, { cwd: repoRoot, encoding: 'utf8' });
  return result.status === 0 ? result.stdout.trim() : '';
}
