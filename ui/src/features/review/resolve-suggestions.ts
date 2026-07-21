import { getSuggestionKey, keyId2SuggestionId, type TResolvedSuggestion } from '@platejs/suggestion';
import { NodeApi, type Descendant } from 'platejs';

import { readBlockSuggestion, readSuggestionMark } from './suggestion-mark';

interface Acc {
  newText: string;
  text: string;
  hasInsert: boolean;
  hasRemove: boolean;
  userId: string;
  createdAt: number;
}

export function resolveSuggestions(value: Descendant[]): TResolvedSuggestion[] {
  const byId = new Map<string, Acc>();
  const visit = (node: Record<string, unknown>, suppressedId?: string) => {
    const block = readBlockSuggestion(node);
    const blockRemoval = block?.type === 'remove' ? block : null;
    if (blockRemoval) {
      byId.set(blockRemoval.id, {
        newText: '',
        text: NodeApi.string(node as never),
        hasInsert: false,
        hasRemove: true,
        userId: blockRemoval.userId,
        createdAt: blockRemoval.createdAt,
      });
    }
    const text = node.text;
    if (typeof text === 'string') {
      const mark = readSuggestionMark(node);
      if (mark && mark.id !== suppressedId) {
        const acc = byId.get(mark.id) ?? {
          newText: '',
          text: '',
          hasInsert: false,
          hasRemove: false,
          userId: mark.userId,
          createdAt: mark.createdAt,
        };
        if (mark.type === 'insert') {
          acc.newText += text;
          acc.hasInsert = true;
        } else if (mark.type === 'remove') {
          acc.text += text;
          acc.hasRemove = true;
        }
        byId.set(mark.id, acc);
      }
    }
    const children = node.children;
    if (Array.isArray(children)) {
      for (const child of children) {
        if (typeof child === 'object' && child !== null) {
          visit({ ...child }, blockRemoval?.id ?? suppressedId);
        }
      }
    }
  };
  for (const node of value) visit({ ...node });

  const out: TResolvedSuggestion[] = [];
  for (const [id, acc] of byId.entries()) {
    const keyId = getSuggestionKey(id);
    const base = { keyId, suggestionId: keyId2SuggestionId(keyId), userId: acc.userId, createdAt: new Date(acc.createdAt) };
    if (acc.hasInsert && acc.hasRemove) out.push({ ...base, type: 'replace', newText: acc.newText, text: acc.text });
    else if (acc.hasInsert) out.push({ ...base, type: 'insert', newText: acc.newText });
    else if (acc.hasRemove) out.push({ ...base, type: 'remove', text: acc.text });
  }
  return out;
}
