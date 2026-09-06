import type { DocumentSaveState } from './document-status';

export type EditorMode = 'editing' | 'suggesting' | 'viewing';
export interface DocumentEditorConfig {
  documentId: string;
  sessionId: string;
  token?: string;
  onSaveStateChange?: (state: DocumentSaveState) => void;
  onReviewOpen?: () => void;
  onTitleChange?: (title: string | null) => void;
}
export interface ImageApi {
  resolveSrc?: (url: string) => string;
  upload?: (file: File) => Promise<string>;
}
export interface WikiResolution { resolved: boolean; targetPath: string | null }
export interface WikiLinkApi {
  resolve?: (target: string) => WikiResolution | undefined;
  open?: (path: string, anchor?: string) => void;
}
