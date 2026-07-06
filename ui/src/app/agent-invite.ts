export function workspaceRouteForDocument(library: string, path: string) {
  if (!library) return '';
  const libraryPath = `/lib/${encodeURIComponent(library)}`;
  if (!path) return libraryPath;
  return `${libraryPath}/documents/${pathSegments(path)}`;
}

export function tmpWorkspaceRouteForDocument(secret: string) {
  if (!secret) return '/tmp';
  return `/tmp/${encodeURIComponent(secret)}`;
}

function pathSegments(path: string) {
  return path.split('/').map(encodeURIComponent).join('/');
}
