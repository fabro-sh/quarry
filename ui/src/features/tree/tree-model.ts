export interface TreeDocument {
  id: string;
  path: string;
  title?: string;
}

export interface TreeNode {
  id: string;
  name: string;
  path: string;
  kind: 'folder' | 'document';
  children?: TreeNode[];
}

interface MutableNode extends TreeNode {
  children?: MutableNode[];
}

export function buildDocumentTree(documents: TreeDocument[]): TreeNode[] {
  const roots: MutableNode[] = [];
  const folders = new Map<string, MutableNode>();

  for (const document of [...documents].sort((a, b) => a.path.localeCompare(b.path))) {
    const parts = document.path.split('/').filter(Boolean);
    let level = roots;
    let prefix = '';

    for (const part of parts.slice(0, -1)) {
      prefix = prefix ? `${prefix}/${part}` : part;
      let folder = folders.get(prefix);
      if (!folder) {
        folder = {
          id: `folder:${prefix}`,
          name: part,
          path: prefix,
          kind: 'folder',
          children: [],
        };
        folders.set(prefix, folder);
        level.push(folder);
      }
      level = folder.children ?? (folder.children = []);
    }

    const fileName = parts.at(-1) ?? document.path;
    level.push({
      id: `doc:${document.path}`,
      name: document.title || fileName,
      path: document.path,
      kind: 'document',
    });
  }

  sortTree(roots);
  return roots;
}

export function flattenTreePaths(nodes: TreeNode[]): string[] {
  const paths: string[] = [];
  for (const node of nodes) {
    paths.push(node.path);
    if (node.children) {
      paths.push(...flattenTreePaths(node.children));
    }
  }
  return paths;
}

export function droppedDocumentPath(document: TreeNode, parent: TreeNode | null) {
  const fileName = document.path.split('/').filter(Boolean).at(-1) ?? document.path;
  if (!parent || parent.path === '') return fileName;
  return `${parent.path}/${fileName}`;
}

function sortTree(nodes: MutableNode[]) {
  nodes.sort((a, b) => {
    if (a.kind !== b.kind) return a.kind === 'folder' ? -1 : 1;
    return a.name.localeCompare(b.name);
  });
  for (const node of nodes) {
    if (node.children) sortTree(node.children);
  }
}
