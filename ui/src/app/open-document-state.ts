import { useCallback, useEffect, useReducer, type Dispatch } from 'react';

import type { LoadedDocument } from '../api/client';
import type { DocumentScope } from './workspace-navigation';

export interface OpenDocumentIdentity {
  readonly documentId: string;
  readonly library: string;
  readonly path: string;
  readonly scope: DocumentScope;
}

interface ClosedDocumentState {
  readonly type: 'closed';
}

interface OpenDocumentState {
  readonly type: 'open';
  readonly compareVersionId: string | null;
  readonly content: string;
  readonly contentType: string;
  readonly currentDiffOpen: boolean;
  readonly etag: string;
  readonly identity: OpenDocumentIdentity;
  readonly selectedVersionId: string | null;
}

type DocumentState = ClosedDocumentState | OpenDocumentState;

type DocumentAction =
  | { readonly type: 'close' }
  | {
      readonly type: 'document-loaded';
      readonly document: LoadedDocument;
      readonly identity: OpenDocumentIdentity;
    }
  | { readonly type: 'mirror-changed'; readonly content: string }
  | { readonly type: 'head-adopted'; readonly etag: string }
  | { readonly type: 'view-version'; readonly versionId: string }
  | { readonly type: 'compare-version'; readonly versionId: string | null }
  | { readonly type: 'open-current-diff' }
  | { readonly type: 'reset-history-view' };

interface OpenDocumentControllerOptions {
  readonly activeLibrary: string;
  readonly document: LoadedDocument | undefined;
  readonly documentScope: DocumentScope;
  readonly selectedPath: string;
}

export interface OpenDocumentController {
  readonly compareVersionId: string | null;
  readonly content: string;
  readonly contentType: string;
  readonly currentDiffOpen: boolean;
  readonly etag: string;
  readonly identity: OpenDocumentIdentity | null;
  readonly selectedVersionId: string | null;
  readonly adoptHead: (etag: string) => void;
  readonly changeCompareVersion: (versionId: string | null) => void;
  readonly changeContent: (content: string) => void;
  readonly diffCurrent: () => void;
  readonly resetHistoryView: () => void;
  readonly viewVersion: (versionId: string) => void;
}

const CLOSED: ClosedDocumentState = { type: 'closed' };

export function useOpenDocumentController({
  activeLibrary,
  document,
  documentScope,
  selectedPath,
}: OpenDocumentControllerOptions): OpenDocumentController {
  const [state, dispatch] = useReducer(reduceDocumentState, CLOSED);

  useLoadedDocumentTransition({
    activeLibrary,
    dispatch,
    document,
    documentScope,
    selectedPath,
  });

  const open = state.type === 'open' ? state : null;
  return {
    compareVersionId: open?.compareVersionId ?? null,
    content: open?.content ?? '',
    contentType: open?.contentType ?? 'text/markdown',
    currentDiffOpen: open?.currentDiffOpen ?? false,
    etag: open?.etag ?? '',
    identity: open?.identity ?? null,
    selectedVersionId: open?.selectedVersionId ?? null,
    adoptHead: useCallback((etag) => dispatch({ type: 'head-adopted', etag }), []),
    changeCompareVersion: useCallback(
      (versionId) => dispatch({ type: 'compare-version', versionId }),
      []
    ),
    changeContent: useCallback(
      (content) => dispatch({ type: 'mirror-changed', content }),
      []
    ),
    diffCurrent: useCallback(() => dispatch({ type: 'open-current-diff' }), []),
    resetHistoryView: useCallback(() => dispatch({ type: 'reset-history-view' }), []),
    viewVersion: useCallback(
      (versionId) => dispatch({ type: 'view-version', versionId }),
      []
    ),
  };
}

interface LoadedDocumentTransitionOptions extends OpenDocumentControllerOptions {
  readonly dispatch: Dispatch<DocumentAction>;
}

function useLoadedDocumentTransition({
  activeLibrary,
  dispatch,
  document,
  documentScope,
  selectedPath,
}: LoadedDocumentTransitionOptions): void {
  useEffect(() => {
    if (!selectedPath) {
      dispatch({ type: 'close' });
      return;
    }
    if (!document || document.path !== selectedPath) return;
    dispatch({
      type: 'document-loaded',
      document,
      identity: {
        documentId: document.documentId,
        library: activeLibrary,
        path: selectedPath,
        scope: documentScope,
      },
    });
  }, [activeLibrary, dispatch, document, documentScope, selectedPath]);
}

export function reduceDocumentState(
  state: DocumentState,
  action: DocumentAction
): DocumentState {
  switch (action.type) {
    case 'close':
      return CLOSED;
    case 'document-loaded': {
      if (state.type === 'open' && sameIdentity(state.identity, action.identity)) {
        return {
          ...state,
          content: isLiveMarkdown(action.document)
            ? state.content
            : action.document.content,
          contentType: action.document.contentType,
          etag: action.document.etag,
        };
      }
      return {
        type: 'open',
        compareVersionId: null,
        content: action.document.content,
        contentType: action.document.contentType,
        currentDiffOpen: false,
        etag: action.document.etag,
        identity: action.identity,
        selectedVersionId: null,
      };
    }
    case 'mirror-changed':
      return state.type === 'open' ? { ...state, content: action.content } : state;
    case 'head-adopted':
      return state.type === 'open' ? { ...state, etag: action.etag } : state;
    case 'view-version':
      return state.type === 'open'
        ? {
            ...state,
            compareVersionId: null,
            currentDiffOpen: false,
            selectedVersionId: action.versionId,
          }
        : state;
    case 'compare-version':
      return state.type === 'open' ? { ...state, compareVersionId: action.versionId } : state;
    case 'open-current-diff':
      return state.type === 'open'
        ? {
            ...state,
            compareVersionId: null,
            currentDiffOpen: true,
            selectedVersionId: null,
          }
        : state;
    case 'reset-history-view':
      return state.type === 'open'
        ? {
            ...state,
            compareVersionId: null,
            currentDiffOpen: false,
            selectedVersionId: null,
          }
        : state;
    default:
      return assertNever(action);
  }
}

function sameIdentity(left: OpenDocumentIdentity, right: OpenDocumentIdentity): boolean {
  return (
    left.documentId === right.documentId &&
    left.library === right.library &&
    left.path === right.path &&
    left.scope === right.scope
  );
}

function isLiveMarkdown(document: LoadedDocument): boolean {
  return (
    document.documentId.length > 0 &&
    document.contentType.split(';', 1)[0]?.trim().toLowerCase() === 'text/markdown'
  );
}

function assertNever(value: never): never {
  throw new Error(`Unexpected document action: ${JSON.stringify(value)}`);
}
