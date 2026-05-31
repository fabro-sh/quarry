import { getSuggestionKey, keyId2SuggestionId, type TResolvedSuggestion } from '@platejs/suggestion';
import type { Descendant } from 'platejs';

import { readSuggestionMark } from './suggestion-mark';

interface Acc { newText: string; text: string; userId: string; createdAt: number; }

export function resolveSuggestions(value: Descendant[]): TResolvedSuggestion[] {
  const byId = new Map<string, Acc>();
  const visit = (node: Record<string, unknown>) => {
    const text = node.text;
    if (typeof text === 'string') {
      const mark = readSuggestionMark(node);
      if (mark) {
        const acc = byId.get(mark.id) ?? { newText: '', text: '', userId: mark.userId, createdAt: mark.createdAt };
        if (mark.type === 'insert') acc.newText += text;
        else if (mark.type === 'remove') acc.text += text;
        byId.set(mark.id, acc);
      }
    }
    const children = node.children;
    if (Array.isArray(children)) {
      for (const child of children) {
        if (typeof child === 'object' && child !== null) visit({ ...child });
      }
    }
  };
  for (const node of value) visit({ ...node });

  const out: TResolvedSuggestion[] = [];
  for (const [id, acc] of byId.entries()) {
    const keyId = getSuggestionKey(id);
    const base = { keyId, suggestionId: keyId2SuggestionId(keyId), userId: acc.userId, createdAt: new Date(acc.createdAt) };
    if (acc.newText && acc.text) out.push({ ...base, type: 'replace', newText: acc.newText, text: acc.text });
    else if (acc.newText) out.push({ ...base, type: 'insert', newText: acc.newText });
    else if (acc.text) out.push({ ...base, type: 'remove', text: acc.text });
  }
  return out;
}
