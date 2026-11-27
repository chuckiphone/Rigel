/**
 * Roadmap 122: drag-and-drop attachments.
 *
 * Every file is checked before a single byte is sent: extension, MIME and size. A refused file
 * never reaches the uploader and never attaches. Progress comes from the uploader's acknowledged
 * byte count, and cancel aborts the in-flight read so the uploader can delete its temporary file.
 */

import { useEffect, useRef, useState, type DragEvent, type ReactNode } from "react";

/** The shell enforces the same cap; keep both numbers in step. */
export const MAX_ATTACHMENT_BYTES = 25 * 1024 * 1024;
export const UPLOAD_CHUNK_BYTES = 256 * 1024;

/** Extensions the composer will hand to the next turn. */
export const ALLOWED_ATTACHMENT_EXTENSIONS = [
  "txt",
  "md",
  "markdown",
  "rs",
  "toml",
  "json",
  "yaml",
  "yml",
  "csv",
  "log",
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "avif",
  "bmp",
  "pdf",
] as const;

/** A MIME type, when the browser supplies one, has to agree with the extension. */
const MIME_BY_EXTENSION: Record<string, string[]> = {
  txt: ["text/plain", "text/", "application/octet-stream"],
  md: ["text/markdown", "text/plain", "text/"],
  markdown: ["text/markdown", "text/plain", "text/"],
  rs: ["text/rust", "text/plain", "text/"],
  toml: ["text/plain", "application/toml", "text/"],
  json: ["application/json", "text/json", "text/"],
  yaml: ["application/yaml", "application/x-yaml", "text/yaml", "text/"],
  yml: ["application/yaml", "application/x-yaml", "text/yaml", "text/"],
  csv: ["text/csv", "text/plain", "text/"],
  log: ["text/plain", "text/"],
  png: ["image/png"],
  jpg: ["image/jpeg"],
  jpeg: ["image/jpeg"],
  gif: ["image/gif"],
  webp: ["image/webp"],
  avif: ["image/avif"],
  bmp: ["image/bmp"],
  pdf: ["application/pdf"],
};

/** The renderer or a test can call this with a `File` or a plain `{name,size,type}` object. */
export type FileLike = Pick<File, "name" | "size" | "type">;

export type Validation = { ok: true } | { ok: false; reason: string };

function refused(reason: string): Validation {
  return { ok: false, reason };
}

export function validateAttachment(file: FileLike): Validation {
  const name = file.name;
  if (!name || name === "." || name === ".." || name.includes("/") || name.includes("\\")) {
    return refused("Unsafe file name");
  }
  const dot = name.lastIndexOf(".");
  const extension = dot < 0 ? "" : name.slice(dot + 1).toLowerCase();
  if (!ALLOWED_ATTACHMENT_EXTENSIONS.some((allowed) => allowed === extension)) {
    return refused(extension ? `.${extension} files cannot be attached` : "Unknown file type");
  }
  if (!Number.isFinite(file.size) || file.size <= 0) {
    return refused("Empty files cannot be attached");
  }
  if (file.size > MAX_ATTACHMENT_BYTES) {
    return refused(
      `${(file.size / (1024 * 1024)).toFixed(1)} MiB exceeds the ${MAX_ATTACHMENT_BYTES / (1024 * 1024)} MiB limit`,
    );
  }
  const mime = file.type.trim().toLowerCase();
  if (mime) {
    const allowed = MIME_BY_EXTENSION[extension] ?? [];
    if (
      !allowed.some((candidate) =>
        candidate.endsWith("/") ? mime.startsWith(candidate) : mime === candidate,
      )
    ) {
      return refused(`${mime} is not a ${extension} file`);
    }
  }
  return { ok: true };
}

/** Validate a whole drop. Refusals are reported with their reason; accepted files still upload. */
export function validateDrop<T extends FileLike>(
  files: T[],
): { accepted: T[]; refused: { name: string; reason: string }[] } {
  const accepted: T[] = [];
  const reasons: { name: string; reason: string }[] = [];
  for (const file of files) {
    const verdict = validateAttachment(file);
    if (verdict.ok) accepted.push(file);
    else reasons.push({ name: file.name, reason: verdict.reason });
  }
  return { accepted, refused: reasons };
}

/** A session-owned upload: reads one file, reports bytes acknowledged by the shell. */
export type AttachmentUpload = (
  file: File,
  signal: AbortSignal,
  onProgress: (sent: number, total: number) => void,
) => Promise<string>;

export default function DropZone({
  children,
  upload,
  onAttached,
  active = true,
  disabled = false,
}: {
  children: ReactNode;
  upload: AttachmentUpload;
  onAttached: (path: string) => void;
  /** Let the composer render the same tree with no drop wrapper when no session is open. */
  active?: boolean;
  disabled?: boolean;
}): ReactNode {
  const [dragging, setDragging] = useState(false);
  const [progress, setProgress] = useState<{ name: string; sent: number; total: number } | null>(
    null,
  );
  const [refusals, setRefusals] = useState<{ name: string; reason: string }[]>([]);
  const controller = useRef<AbortController | null>(null);

  // A composer that unmounts mid-upload must not leave a temporary file behind.
  useEffect(() => () => controller.current?.abort(), []);

  if (!active) return <>{children}</>;

  const onDrop = (event: DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    setDragging(false);
    if (disabled || controller.current) return;
    const { accepted, refused } = validateDrop(Array.from(event.dataTransfer.files));
    setRefusals(refused);
    const run = async () => {
      for (const file of accepted) {
        const abort = new AbortController();
        controller.current = abort;
        setProgress({ name: file.name, sent: 0, total: file.size });
        try {
          const path = await upload(file, abort.signal, (sent, total) =>
            setProgress({ name: file.name, sent, total }),
          );
          if (!abort.signal.aborted) {
            onAttached(path);
            setRefusals([]);
          }
        } catch (error) {
          if (!abort.signal.aborted) {
            setRefusals([
              {
                name: file.name,
                reason: error instanceof Error ? error.message : String(error),
              },
            ]);
          }
        } finally {
          controller.current = null;
          setProgress(null);
        }
      }
    };
    void run();
  };

  const percent =
    progress && progress.total > 0 ? Math.round((progress.sent / progress.total) * 100) : 0;

  return (
    <div
      className={`composer-dropzone${dragging ? " dragging" : ""}`}
      onDragEnter={(event) => {
        event.preventDefault();
        if (!disabled) setDragging(true);
      }}
      onDragOver={(event) => {
        event.preventDefault();
        if (!disabled) setDragging(true);
      }}
      onDragLeave={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDragging(false);
      }}
      onDrop={onDrop}
    >
      {children}
      {dragging ? (
        <p className="composer-drop-hint" role="status">
          Drop files to attach them to the next turn
        </p>
      ) : null}
      {progress ? (
        <div className="composer-upload" role="status" aria-label={`Uploading ${progress.name}`}>
          <span className="composer-upload-name">{progress.name}</span>
          <progress value={progress.sent} max={progress.total} aria-label={`Uploading ${progress.name}`} />
          <span className="composer-upload-value">{percent}%</span>
          <button
            type="button"
            className="composer-upload-cancel"
            onClick={() => controller.current?.abort()}
          >
            Cancel upload
          </button>
        </div>
      ) : null}
      {refusals.map((refusal) => (
        <p className="composer-drop-refusal" role="alert" key={`${refusal.name}-${refusal.reason}`}>
          {refusal.name}: {refusal.reason}
        </p>
      ))}
    </div>
  );
}
