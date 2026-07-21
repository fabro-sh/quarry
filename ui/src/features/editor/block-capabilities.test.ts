import { describe, expect, it } from 'vitest';

import {
  blockCapabilities,
  canPromoteFullTextDelete,
  KNOWN_BLOCK_TYPES,
  usesLiteralInlineSyntax,
} from './block-capabilities';

describe('block capability registry', () => {
  it('defines the shared API vocabulary without duplicate types', () => {
    expect(new Set(KNOWN_BLOCK_TYPES).size).toBe(KNOWN_BLOCK_TYPES.length);
    expect(KNOWN_BLOCK_TYPES).toEqual([
      'p',
      'h1',
      'h2',
      'h3',
      'h4',
      'h5',
      'h6',
      'blockquote',
      'code_block',
      'code_line',
      'mermaid',
      'table',
      'tr',
      'th',
      'td',
      'img',
      'hr',
      'raw_markdown',
    ]);
  });

  it('drives syntax and full-text-delete behavior by capability', () => {
    expect(blockCapabilities('p')?.content).toBe('text');
    expect(blockCapabilities('table')?.content).toBe('container');
    expect(blockCapabilities('hr')?.content).toBe('void');
    expect(blockCapabilities('raw_markdown')?.content).toBe('raw');
    expect(usesLiteralInlineSyntax('code_line')).toBe(true);
    expect(usesLiteralInlineSyntax('a')).toBe(false);
    expect(canPromoteFullTextDelete('blockquote')).toBe(true);
    expect(canPromoteFullTextDelete('img')).toBe(false);
    expect(blockCapabilities('future_block')).toBeUndefined();
  });
});
