/**
 * Roadmap 137: composer v2.
 *
 * The controls that shape a turn live in one row above the draft: attachments, pinned skills, the
 * model and its effort, a context meter and the act/plan toggle. Labels are single-line text and
 * the row wraps between controls, never inside one, so the narrowest supported window keeps every
 * control readable. Every control is a native button or select and therefore keyboard reachable.
 */

import { useId, type ReactNode } from "react";
import DropZone, { type AttachmentUpload } from "./composer/dropzone";

export { default as DropZone } from "./composer/dropzone";
export type { AttachmentUpload } from "./composer/dropzone";

/** The design contract's minimum window width; the test checks this row at 320px. */
export const MIN_COMPOSER_WIDTH = 320;

export type ComposerMode = "act" | "plan";

export const EFFORT_OPTIONS = ["low", "medium", "high"] as const;

/** Stable names, so a test can prove all six controls are present in one row. */
export const COMPOSER_CONTROLS = [
  "attachments",
  "skills",
  "model",
  "effort",
  "context",
  "mode",
] as const;

function Control({
  label,
  value,
  title,
  onClick,
  disabled,
}: {
  label: string;
  value?: string;
  title: string;
  onClick?: () => void;
  disabled?: boolean;
}): ReactNode {
  // Deriving the accessible name from the visible spans keeps axe's name/content rule satisfied
  // while the title still explains what the control does.
  const id = useId();
  return (
    <button
      type="button"
      className="composer-v2-control"
      aria-labelledby={`${id}-label${value ? ` ${id}-value` : ""}`}
      title={title}
      onClick={onClick}
      disabled={disabled}
    >
      <span className="composer-v2-label" id={`${id}-label`}>
        {label}
      </span>
      {value ? (
        <>
          {" "}
          <span className="composer-v2-value mono" id={`${id}-value`}>
            {value}
          </span>
        </>
      ) : null}
    </button>
  );
}

export default function ComposerBar({
  attachments,
  skills,
  model,
  provider,
  effort,
  contextPercent,
  mode,
  disabled = false,
  upload,
  onAttached,
  onAttachments,
  onSkills,
  onModel,
  onEffort,
  onContext,
  onMode,
}: {
  attachments: number;
  skills: number;
  model: string;
  provider?: string;
  effort: string;
  contextPercent: number | null;
  mode: ComposerMode;
  disabled?: boolean;
  /** Roadmap 122: when present, the bar is the drop target for files to attach. */
  upload?: AttachmentUpload;
  onAttached?: (path: string) => void;
  onAttachments?: () => void;
  onSkills?: () => void;
  onModel?: () => void;
  onEffort?: (effort: string) => void;
  onContext?: () => void;
  onMode?: (mode: ComposerMode) => void;
}) {
  const pair = provider ? `${provider} / ${model}` : model;
  return (
    <DropZone
      upload={upload ?? (async () => "")}
      onAttached={onAttached ?? (() => {})}
      active={Boolean(upload)}
      disabled={disabled}
    >
    <div className="composer-v2" role="group" aria-label="Composer controls">
      <Control
        label="Attach"
        value={String(attachments)}
        title="Attach project files to the next turn"
        onClick={onAttachments}
        disabled={disabled}
      />
      <Control
        label="Skills"
        value={String(skills)}
        title="Pinned skills for this session"
        onClick={onSkills}
        disabled={disabled}
      />
      <Control
        label="Model"
        value={model.split("/").pop() ?? model}
        title={`Model for the next turn: ${pair}`}
        onClick={onModel}
        disabled={disabled}
      />
      <label className="composer-v2-control composer-v2-effort">
        <span className="composer-v2-label">Effort</span>
        <select
          aria-label="Composer effort"
          value={effort}
          disabled={disabled}
          onChange={(event) => onEffort?.(event.target.value)}
        >
          {EFFORT_OPTIONS.map((value) => (
            <option key={value} value={value}>
              {value}
            </option>
          ))}
        </select>
      </label>
      <Control
        label="Context"
        value={contextPercent === null ? "?" : `${Math.round(contextPercent)}%`}
        title={
          contextPercent === null
            ? "No published context window for this model"
            : `${Math.round(contextPercent)}% of the window used`
        }
        onClick={onContext}
        disabled={disabled}
      />
      <div className="composer-v2-mode" role="group" aria-label="Composer mode v2">
        {(["act", "plan"] as const).map((value) => (
          <button
            key={value}
            type="button"
            className="composer-v2-control"
            aria-label={`${value === "act" ? "Act" : "Plan"} mode`}
            aria-pressed={mode === value}
            title={
              value === "act" ? "The next turn may edit" : "Reads only, until a plan is approved"
            }
            disabled={disabled}
            onClick={() => onMode?.(value)}
          >
            <span className="composer-v2-label">{value === "act" ? "Act" : "Plan"}</span>
          </button>
        ))}
      </div>
    </div>
    </DropZone>
  );
}
