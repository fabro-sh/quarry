import type { Descendant, TElement } from 'platejs';

import { documentHref } from '../../api/client';

// Dropped images are stored as content-addressed documents under `assets/` and
// referenced with ordinary `![](assets/<hash>.<ext>)` markdown. The img node
// keeps the relative path (portable, and resolvable by the backend's link
// index); only rendering turns it into a serve URL.

const TYPE_EXT: Record<string, string> = {
  'image/png': 'png',
  'image/jpeg': 'jpg',
  'image/gif': 'gif',
  'image/webp': 'webp',
  'image/svg+xml': 'svg',
  'image/avif': 'avif',
  'image/bmp': 'bmp',
};

export function imageExtension(type: string, name: string): string {
  const fromType = TYPE_EXT[type.split(';', 1)[0]?.trim().toLowerCase() ?? ''];
  if (fromType) return fromType;
  const fromName = name.includes('.') ? name.split('.').pop()?.toLowerCase() : undefined;
  return fromName || 'bin';
}

/** The content-addressed asset path for a dropped file (SHA-256 of its bytes). */
export async function imageAssetPath(file: File): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', await file.arrayBuffer());
  const hash = Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('');
  return `assets/${hash}.${imageExtension(file.type, file.name)}`;
}

/** Resolve an img node's url to a renderable src: relative paths hit the serve
 * endpoint; absolute/data/blob urls pass through. */
export function resolveImageSrc(url: string, library: string): string {
  if (/^(?:https?:|data:|blob:)/i.test(url)) return url;
  return documentHref(library, url);
}

/** Drop transient upload placeholders before serializing — they aren't part of
 * the saved document (the image lands once its upload finishes). */
export function stripPlaceholders(value: Descendant[]): Descendant[] {
  const out: Descendant[] = [];
  for (const node of value) {
    if ((node as TElement).type === 'placeholder') continue;
    const children = (node as TElement).children;
    out.push(Array.isArray(children) ? { ...node, children: stripPlaceholders(children) } : node);
  }
  return out;
}
