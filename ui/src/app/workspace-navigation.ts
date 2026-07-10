import { useEffect, useMemo } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';

import type { Library } from '../api/generated/types';
import { tmpWorkspaceRouteForDocument, workspaceRouteForDocument } from './agent-invite';

export type DocumentScope = 'library' | 'tmp';

export interface WorkspaceRouteSelection {
  readonly scope: DocumentScope;
  readonly library: string | null;
  readonly path: string | undefined;
  readonly createTmp: boolean;
}

interface WorkspaceNavigationOptions {
  readonly capabilitiesLoaded: boolean;
  readonly defaultLibrary: string;
  readonly libDocumentsEnabled: boolean;
  readonly libraries: readonly Library[];
  readonly tmpDocumentsEnabled: boolean;
}

export interface WorkspaceNavigation {
  readonly activeLibrary: string;
  readonly documentScope: DocumentScope;
  readonly routeSelection: WorkspaceRouteSelection;
  readonly selectedPath: string;
  readonly closeDocument: () => void;
  readonly openDocument: (path: string) => void;
  readonly openLibraryDocument: (
    library: string,
    path: string,
    options?: WorkspaceNavigationOptionsArg
  ) => void;
  readonly openTmpDocument: (secret: string, options?: WorkspaceNavigationOptionsArg) => void;
  readonly selectLibrary: (library: string) => void;
}

interface WorkspaceNavigationOptionsArg {
  readonly replace?: boolean;
}

export function useWorkspaceNavigation({
  capabilitiesLoaded,
  defaultLibrary,
  libDocumentsEnabled,
  libraries,
  tmpDocumentsEnabled,
}: WorkspaceNavigationOptions): WorkspaceNavigation {
  const location = useLocation();
  const navigate = useNavigate();
  const routeSelection = useMemo(
    () => parseWorkspaceRoute(location.pathname),
    [location.pathname]
  );
  const savedLibrary = readSavedLibrary();
  const knownLibraries = new Set(libraries.map((library) => library.slug));
  const routeLibrary = routeSelection.library;
  const activeLibrary =
    routeLibrary && (knownLibraries.size === 0 || knownLibraries.has(routeLibrary))
      ? routeLibrary
      : savedLibrary && (knownLibraries.size === 0 || knownLibraries.has(savedLibrary))
        ? savedLibrary
        : defaultLibrary;
  const selectedPath = routeSelection.path ?? '';

  useCanonicalWorkspaceRoute({
    activeLibrary,
    capabilitiesLoaded,
    libDocumentsEnabled,
    locationPathname: location.pathname,
    navigate,
    routeSelection,
    tmpDocumentsEnabled,
  });

  return useMemo(
    () => ({
      activeLibrary,
      documentScope: routeSelection.scope,
      routeSelection,
      selectedPath,
      closeDocument: () => {
        navigate(
          routeSelection.scope === 'tmp'
            ? tmpWorkspaceRouteForDocument('')
            : workspaceRouteForDocument(activeLibrary, '')
        );
      },
      openDocument: (path: string) => {
        navigate(
          routeSelection.scope === 'tmp'
            ? tmpWorkspaceRouteForDocument(path)
            : workspaceRouteForDocument(activeLibrary, path)
        );
      },
      openLibraryDocument: (
        library: string,
        path: string,
        options?: WorkspaceNavigationOptionsArg
      ) => {
        navigate(workspaceRouteForDocument(library, path), { replace: options?.replace });
      },
      openTmpDocument: (secret: string, options?: WorkspaceNavigationOptionsArg) => {
        navigate(tmpWorkspaceRouteForDocument(secret), { replace: options?.replace });
      },
      selectLibrary: (library: string) => {
        navigate(workspaceRouteForDocument(library, ''));
      },
    }),
    [activeLibrary, navigate, routeSelection, selectedPath]
  );
}

interface CanonicalWorkspaceRouteOptions {
  readonly activeLibrary: string;
  readonly capabilitiesLoaded: boolean;
  readonly libDocumentsEnabled: boolean;
  readonly locationPathname: string;
  readonly navigate: ReturnType<typeof useNavigate>;
  readonly routeSelection: WorkspaceRouteSelection;
  readonly tmpDocumentsEnabled: boolean;
}

function useCanonicalWorkspaceRoute({
  activeLibrary,
  capabilitiesLoaded,
  libDocumentsEnabled,
  locationPathname,
  navigate,
  routeSelection,
  tmpDocumentsEnabled,
}: CanonicalWorkspaceRouteOptions): void {
  useEffect(() => {
    if (!capabilitiesLoaded || routeSelection.createTmp) return;

    if (routeSelection.scope === 'library' && !libDocumentsEnabled && tmpDocumentsEnabled) {
      navigate('/tmp', { replace: true });
      return;
    }
    if (routeSelection.scope === 'tmp' && !tmpDocumentsEnabled && libDocumentsEnabled) {
      const next = workspaceRouteForDocument(activeLibrary, '');
      if (next) navigate(next, { replace: true });
      return;
    }
    if (routeSelection.scope !== 'library' || !libDocumentsEnabled || !activeLibrary) return;

    const routeLibrary = routeSelection.library;
    if (routeLibrary === activeLibrary) return;
    const next = workspaceRouteForDocument(activeLibrary, routeSelection.path ?? '');
    if (next && next !== locationPathname) navigate(next, { replace: true });
  }, [
    activeLibrary,
    capabilitiesLoaded,
    libDocumentsEnabled,
    locationPathname,
    navigate,
    routeSelection,
    tmpDocumentsEnabled,
  ]);
}

export function parseWorkspaceRoute(pathname: string): WorkspaceRouteSelection {
  const segments = pathname.split('/').filter(Boolean);
  if (segments[0] === 'tmp') {
    if (segments[1] === 'new') {
      return { scope: 'tmp', library: null, path: '', createTmp: true };
    }
    return {
      scope: 'tmp',
      library: null,
      path: segments[1] ? safeDecodeSegment(segments[1]) : '',
      createTmp: false,
    };
  }
  if (segments[0] !== 'lib' || !segments[1]) {
    return { scope: 'library', library: null, path: undefined, createTmp: false };
  }
  const library = safeDecodeSegment(segments[1]);
  if (segments[2] !== 'documents') {
    return { scope: 'library', library, path: '', createTmp: false };
  }
  return {
    scope: 'library',
    library,
    path: segments.slice(3).map(safeDecodeSegment).join('/'),
    createTmp: false,
  };
}

function readSavedLibrary(): string {
  try {
    return localStorage.getItem('quarry:active-library') ?? '';
  } catch {
    return '';
  }
}

function safeDecodeSegment(segment: string): string {
  try {
    return decodeURIComponent(segment);
  } catch {
    return segment;
  }
}
