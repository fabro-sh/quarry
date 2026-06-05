// Brand colors for known agent providers, used to tint each avatar. The logo
// SVG in /agent-icons/<kind>.svg carries the same color, so an unknown kind
// (no entry here) falls back to a neutral avatar rather than a 404.
export const AGENT_BRANDS: Record<string, string> = {
  claude: '#D97757',
  codex: '#000000',
  copilot: '#000000',
  cursor: '#000000',
  gemini: '#8E75B2',
  grok: '#0A0A0A',
  mistral: '#FA520F',
  ollama: '#000000',
  perplexity: '#1FB8CD',
};

// Resolve a free-form author label to a known agent provider, or null for a
// human. Two shapes appear in the wild:
//   - structured ids like "ai:claude:abc" — the provider is the segment after
//     "ai:" (or the first segment), and only that segment decides the icon;
//   - display names like "Claude Sonnet" — match a known provider as a word.
export function agentKind(label: string): string | null {
  const trimmed = label.trim();
  if (!trimmed) return null;

  if (trimmed.includes(':')) {
    const parts = trimmed.split(':').filter(Boolean);
    const provider = (parts[0] === 'ai' ? parts[1] : parts[0])?.toLowerCase() ?? '';
    return provider in AGENT_BRANDS ? provider : null;
  }

  const words = trimmed.toLowerCase().split(/[^a-z0-9]+/).filter(Boolean);
  return words.find((word) => word in AGENT_BRANDS) ?? null;
}

export function agentBrand(kind: string): string | undefined {
  return AGENT_BRANDS[kind];
}

export function agentIconSrc(kind: string): string {
  return `/agent-icons/${kind}.svg`;
}
