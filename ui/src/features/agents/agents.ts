// Known agent providers. Each has a brand logo at /agent-icons/<kind>.svg; an
// unknown kind (not in this set) falls back to a neutral avatar rather than a 404.
const AGENT_PROVIDERS = new Set([
  'claude',
  'codex',
  'copilot',
  'cursor',
  'gemini',
  'grok',
  'mistral',
  'ollama',
  'perplexity',
]);

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
    return AGENT_PROVIDERS.has(provider) ? provider : null;
  }

  const words = trimmed.toLowerCase().split(/[^a-z0-9]+/).filter(Boolean);
  return words.find((word) => AGENT_PROVIDERS.has(word)) ?? null;
}

export function agentIconSrc(kind: string): string {
  return `/agent-icons/${kind}.svg`;
}
