import { describe, expect, it } from 'vitest';

import { agentKind } from './agents';

describe('agentKind', () => {
  it('resolves a structured agent id to its provider segment', () => {
    expect(agentKind('ai:claude:abc123')).toBe('claude');
    expect(agentKind('ai:codex')).toBe('codex');
    expect(agentKind('gemini:xyz')).toBe('gemini');
  });

  it('ignores non-provider segments of a structured id', () => {
    // The provider is the segment after `ai:`; trailing id segments never pick the icon.
    expect(agentKind('ai:unknown:gemini-helper')).toBeNull();
  });

  it('resolves a display name to a known provider by word', () => {
    expect(agentKind('Claude Sonnet')).toBe('claude');
    expect(agentKind('Claude')).toBe('claude');
    expect(agentKind('Gemini 2.5 Pro')).toBe('gemini');
  });

  it('returns null for humans and unknown authors', () => {
    expect(agentKind('user')).toBeNull();
    expect(agentKind('reviewer')).toBeNull();
    expect(agentKind('AI')).toBeNull();
    expect(agentKind('Avery')).toBeNull();
    expect(agentKind('')).toBeNull();
  });

  it('matches whole words only, not substrings', () => {
    expect(agentKind('Claudia')).toBeNull();
  });
});
