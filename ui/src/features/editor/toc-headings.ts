import { ElementApi, NodeApi, type Descendant, type Path } from 'platejs';

/** Read heading text without walking the text leaves of every paragraph. */
export function documentHeadings(children: Descendant[]) {
  const headings: { id: string; depth: number; path: Path; title: string; type: string }[] = [];
  const visit = (nodes: Descendant[], parent: Path) => {
    nodes.forEach((node, index) => {
      if (!ElementApi.isElement(node)) return;
      const heading = /^h([1-6])$/.exec(node.type);
      if (heading) {
        const title = NodeApi.string(node);
        if (title) headings.push({ id: String(node.id), depth: Number(heading[1]), path: [...parent, index], title, type: node.type });
      }
      if (node.children.some(ElementApi.isElement)) visit(node.children, [...parent, index]);
    });
  };
  visit(children, []);
  return headings;
}
