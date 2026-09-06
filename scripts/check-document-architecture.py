#!/usr/bin/env python3
"""Reject retired document engines and mutation paths in application sources."""
from pathlib import Path
import json
import re
import sys

root = Path(__file__).resolve().parents[1]
retired = re.compile(r'@slate-yjs/|[\"\'](?:yjs|yrs|y-protocols)[\"\']|yrs::|quarry-collab-codec|session_doc::|mirror-serializer|prosemirror-|ProseMirror|/v1/(?:tmp/)?collab/')
files = [root / 'Cargo.toml', root / 'Cargo.lock', root / 'ui/package.json', root / 'ui/bun.lock', root / 'ui/vite.config.ts']
files.extend(root / name for name in ['README.md', 'docs/architecture.md', 'docs/development.md', 'docs/manual-test-plan.md', 'docs/security/threat-model.md', 'spec-browser.md'])
for directory in ['crates', 'ui/src', 'ui/tests', 'ui/scripts']:
    files.extend(path for path in (root / directory).rglob('*') if path.suffix in {'.rs', '.ts', '.tsx', '.md', '.toml'})
failures = []
for path in files:
    for line, text in enumerate(path.read_text().splitlines(), 1):
        # HTTP conformance deliberately probes removed routes and rejects them.
        if 'tests/rest_document_sessions.rs' in str(path) and ('/collab/' in text or '/tmp/collab/' in text):
            continue
        if retired.search(text):
            failures.append(f'{path.relative_to(root)}:{line}: retired document architecture reference')
for path in ['crates/quarry-collab-codec', 'crates/quarry-server/src/session.rs', 'crates/quarry-server/src/collab.rs', 'crates/quarry-server/src/collab_handlers.rs', 'ui/src/features/collab',
             *[f'ui/src/features/editor/{name}' for name in ['document-editor.ts', 'document-editor.css', 'document-editor.test.ts', 'document-schema.ts', 'document-clipboard.ts', 'document-node-views.ts', 'document-preview.ts', 'document-tool-menu.tsx']]]:
    if (root / path).exists() and ((root / path).is_file() or any((root / path).rglob('*'))):
        failures.append(f'{path}: retired module exists')
manifest = json.loads((root / 'ui/package.json').read_text())
if not (root / 'crates/quarry-document').is_dir() or 'document:build' not in manifest['scripts']:
    failures.append('native document engine is missing')
if 'platejs' not in manifest['dependencies'] or not (root / 'ui/src/features/editor/plate-document-adapter.ts').is_file():
    failures.append('Plate input adapter is missing')
if failures:
    print('\n'.join(failures), file=sys.stderr)
    raise SystemExit(1)
print('Native document architecture inventory passed')
