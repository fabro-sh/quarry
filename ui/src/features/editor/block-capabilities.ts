import manifest from '../../../../crates/quarry-collab-codec/block-capabilities.json';

export type BlockContentModel = 'text' | 'container' | 'void' | 'raw';
export type InlineSyntax = 'parsed' | 'literal';

export interface BlockCapabilities {
  type: string;
  content: BlockContentModel;
  inlineSyntax: InlineSyntax;
  promoteFullTextDelete: boolean;
}

function readRegistry(value: unknown): readonly BlockCapabilities[] {
  if (
    !Array.isArray(value) ||
    !value.every(
      (entry): entry is BlockCapabilities =>
        typeof entry === 'object' &&
        entry !== null &&
        typeof entry.type === 'string' &&
        ['text', 'container', 'void', 'raw'].includes(entry.content) &&
        (entry.inlineSyntax === 'parsed' || entry.inlineSyntax === 'literal') &&
        typeof entry.promoteFullTextDelete === 'boolean'
    )
  ) {
    throw new Error('block capability manifest is invalid');
  }
  return value;
}

const registry = readRegistry(manifest);
const byType = new Map(registry.map((entry) => [entry.type, entry]));

if (byType.size !== registry.length) {
  throw new Error('block capability manifest contains duplicate types');
}

export const KNOWN_BLOCK_TYPES = registry.map((entry) => entry.type) as readonly string[];

export function blockCapabilities(blockType: unknown): BlockCapabilities | undefined {
  return typeof blockType === 'string' ? byType.get(blockType) : undefined;
}

export function usesLiteralInlineSyntax(blockType: unknown): boolean {
  return blockCapabilities(blockType)?.inlineSyntax === 'literal';
}

export function canPromoteFullTextDelete(blockType: unknown): boolean {
  return blockCapabilities(blockType)?.promoteFullTextDelete === true;
}
