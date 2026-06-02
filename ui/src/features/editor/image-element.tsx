import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from 'react';
import { ImagePlugin, PlaceholderPlugin, PlaceholderProvider } from '@platejs/media/react';
import { ImageOff, Loader2 } from 'lucide-react';
import { KEYS, type TImageElement, type TPlaceholderElement } from 'platejs';
import {
  PlateElement,
  useEditorPlugin,
  useSelected,
  withHOC,
  type PlateElementProps,
} from 'platejs/react';

import { cn } from '../../lib/utils';

export interface ImageApi {
  // Resolve an img node's url (a relative asset path) to a renderable src.
  resolveSrc?: (url: string) => string;
  // Upload a dropped/pasted file and return its relative asset path.
  upload?: (file: File) => Promise<string>;
}

const ImageContext = createContext<ImageApi>({});

export function ImageProvider({ value, children }: { value: ImageApi; children: ReactNode }) {
  return <ImageContext.Provider value={value}>{children}</ImageContext.Provider>;
}

const useImageApi = () => useContext(ImageContext);

function imageAlt(element: TImageElement): string {
  const caption = (element as { caption?: Array<{ text?: string }> }).caption;
  return caption?.map((node) => node.text ?? '').join('') ?? '';
}

export function ImageElement(props: PlateElementProps<TImageElement>) {
  const { resolveSrc } = useImageApi();
  const selected = useSelected();
  const url = props.element.url;
  return (
    <PlateElement {...props} className="py-1.5">
      <figure className="m-0" contentEditable={false}>
        <img
          alt={imageAlt(props.element)}
          className={cn(
            'block max-w-full rounded-sm',
            selected && 'ring-2 ring-accent ring-offset-2 ring-offset-surface'
          )}
          src={resolveSrc ? resolveSrc(url) : url}
        />
      </figure>
      {props.children}
    </PlateElement>
  );
}

function useImageUpload() {
  const { upload } = useImageApi();
  const [uploadedUrl, setUploadedUrl] = useState<string | undefined>();
  const [uploadingFile, setUploadingFile] = useState<File | undefined>();
  const [failed, setFailed] = useState(false);
  const uploadFile = useCallback(
    async (file: File) => {
      if (!upload) return;
      setUploadingFile(file);
      setFailed(false);
      try {
        setUploadedUrl(await upload(file));
      } catch {
        setFailed(true);
      }
    },
    [upload]
  );
  return { failed, uploadFile, uploadedUrl, uploadingFile };
}

// Transient node shown while a dropped/pasted image uploads; replaced by an img
// node once the upload resolves.
export const PlaceholderElement = withHOC(
  PlaceholderProvider,
  function PlaceholderElement(props: PlateElementProps<TPlaceholderElement>) {
    const { editor, element } = props;
    const { api } = useEditorPlugin(PlaceholderPlugin);
    const { failed, uploadFile, uploadedUrl, uploadingFile } = useImageUpload();
    const started = useRef(false);

    useEffect(() => {
      if (started.current) return;
      started.current = true;
      const file = api.placeholder.getUploadingFile(element.id as string);
      if (file) void uploadFile(file);
    }, [api.placeholder, element.id, uploadFile]);

    useEffect(() => {
      if (!uploadedUrl) return;
      const path = editor.api.findPath(element);
      if (!path) return;
      editor.tf.withoutNormalizing(() => {
        editor.tf.removeNodes({ at: path });
        editor.tf.insertNodes({ type: KEYS.img, url: uploadedUrl, children: [{ text: '' }] }, { at: path });
      });
      api.placeholder.removeUploadingFile(element.id as string);
    }, [uploadedUrl, api.placeholder, editor, element]);

    return (
      <PlateElement className="my-1.5" {...props}>
        <div
          className="flex items-center gap-2 rounded-sm bg-well px-3 py-2 text-sm text-muted"
          contentEditable={false}
          data-testid="image-placeholder"
        >
          {failed ? (
            <>
              <ImageOff className="size-4 text-danger" />
              Upload failed
            </>
          ) : (
            <>
              <Loader2 className="size-4 animate-spin" />
              Uploading {uploadingFile?.name ?? 'image'}…
            </>
          )}
        </div>
        {props.children}
      </PlateElement>
    );
  }
);

export const ImageKit = [
  ImagePlugin.configure({ options: { disableUploadInsert: true }, render: { node: ImageElement } }),
  PlaceholderPlugin.configure({
    options: { disableFileDrop: true },
    render: { node: PlaceholderElement },
  }),
];
