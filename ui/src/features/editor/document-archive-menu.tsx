import { useRef, useState } from 'react';
import * as DropdownMenu from '@radix-ui/react-dropdown-menu';
import { MoreHorizontal } from 'lucide-react';
import { DocumentModel } from './document-model';
import type { DocumentSession } from './document-session';

export function download(name: string, text: string) {
  const url = URL.createObjectURL(new Blob([text], { type: 'application/json' }));
  const link = window.document.createElement('a'); link.href = url; link.download = name; link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export function DocumentArchiveMenu({ session, documentUrl, onError }: { session: DocumentSession; documentUrl: string; onError(error: unknown): void }) {
  const input = useRef<HTMLInputElement>(null);
  const [imported, setImported] = useState<string>();
  return <div className="absolute top-2 right-3 z-20 flex items-center gap-2">
    {imported && <a href={imported} className="text-xs text-accent-ink">Open imported document</a>}
    <input type="file" ref={input} hidden accept=".quarry,application/json" aria-label="Import review archive" onChange={(event) => {
      const file = event.target.files?.[0]; event.target.value = ''; if (!file) return;
      void (async () => {
        const text = await file.text(); const checked = DocumentModel.fromArchive(text); checked.dispose();
        const id = crypto.randomUUID().replaceAll('-', '');
        const library = documentUrl.match(/^(.*\/v1\/libraries\/([^/]+))\/documents\//);
        const url = library ? `${library[1]}/documents/imported-${id}.md/archive` : `/v1/tmp/documents/${id}/archive`;
        const response = await fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: text });
        if (!response.ok) throw new Error((await response.json()).message ?? 'Archive import failed');
        setImported(library ? `/lib/${library[2]}/documents/imported-${id}.md` : `/tmp/${id}`);
      })().catch(onError);
    }} />
    <DropdownMenu.Root><DropdownMenu.Trigger asChild><button type="button" aria-label="Document actions" className="rounded p-1 text-faint hover:bg-well hover:text-body"><MoreHorizontal size={18} /></button></DropdownMenu.Trigger>
      <DropdownMenu.Portal><DropdownMenu.Content align="end" sideOffset={6} className="z-50 rounded-md border border-line bg-raised p-1 text-sm shadow-lg">
        <DropdownMenu.Item className="cursor-pointer rounded px-3 py-2 text-body outline-none data-highlighted:bg-well" onSelect={() => input.current?.click()}>Import review archive</DropdownMenu.Item>
        <DropdownMenu.Item className="cursor-pointer rounded px-3 py-2 text-body outline-none data-highlighted:bg-well" onSelect={() => download('document.quarry', session.model.archive(session.metadata))}>Download review archive</DropdownMenu.Item>
      </DropdownMenu.Content></DropdownMenu.Portal>
    </DropdownMenu.Root>
  </div>;
}
