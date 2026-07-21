export interface SuggestionMark {
  id: string;
  type: 'insert' | 'remove' | 'update';
  userId: string;
  createdAt: number;
}

export function readSuggestionMark(leaf: Record<string, unknown>): SuggestionMark | null {
  for (const key of Object.keys(leaf)) {
    if (!key.startsWith('suggestion_')) continue;
    const raw = leaf[key];
    if (typeof raw !== 'object' || raw === null) continue;
    const data: Record<string, unknown> = { ...raw };
    const { id, type, userId, createdAt } = data;
    if (typeof id === 'string' && (type === 'insert' || type === 'remove' || type === 'update')) {
      return { id, type, userId: typeof userId === 'string' ? userId : 'user', createdAt: typeof createdAt === 'number' ? createdAt : 0 };
    }
  }
  return null;
}

/** Read Plate's element-level representation of a block suggestion. */
export function readBlockSuggestion(node: Record<string, unknown>): SuggestionMark | null {
  const raw = node.suggestion;
  if (typeof raw !== 'object' || raw === null) return null;
  const data: Record<string, unknown> = { ...raw };
  const { id, type, userId, createdAt } = data;
  if (
    typeof id !== 'string' ||
    (type !== 'insert' && type !== 'remove' && type !== 'update')
  ) {
    return null;
  }
  return {
    id,
    type,
    userId: typeof userId === 'string' ? userId : 'user',
    createdAt: typeof createdAt === 'number' ? createdAt : 0,
  };
}
