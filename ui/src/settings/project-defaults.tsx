import { useEffect, useState } from "react";

export type ApprovalDefault = "once" | "session" | "project" | "always";

export type ProjectDefaults = {
  model: string;
  effort: "low" | "medium" | "high";
  max_steps: number;
  approval_scope: ApprovalDefault;
};

export const DEFAULT_PROJECT_DEFAULTS: ProjectDefaults = {
  model: "gpt-5.6-sol-medium",
  effort: "medium",
  max_steps: 24,
  approval_scope: "once",
};

const STORAGE_PREFIX = "rigel.project-defaults.v1:";
const SESSION_PREFIX = "rigel.session-defaults.v1:";

export function projectDefaultsKey(project: string): string {
  return `${STORAGE_PREFIX}${project}`;
}

export function loadProjectDefaults(project: string): ProjectDefaults {
  try {
    const saved = localStorage.getItem(projectDefaultsKey(project));
    if (!saved) return { ...DEFAULT_PROJECT_DEFAULTS };
    const parsed = JSON.parse(saved) as Partial<ProjectDefaults>;
    return normalizeProjectDefaults(parsed);
  } catch {
    return { ...DEFAULT_PROJECT_DEFAULTS };
  }
}

export function saveProjectDefaults(project: string, defaults: ProjectDefaults): void {
  localStorage.setItem(projectDefaultsKey(project), JSON.stringify(normalizeProjectDefaults(defaults)));
}

export function saveSessionDefaults(session: string, defaults: ProjectDefaults): void {
  localStorage.setItem(`${SESSION_PREFIX}${session}`, JSON.stringify(normalizeProjectDefaults(defaults)));
}

export function loadSessionDefaults(session: string): ProjectDefaults | null {
  try {
    const saved = localStorage.getItem(`${SESSION_PREFIX}${session}`);
    return saved ? normalizeProjectDefaults(JSON.parse(saved) as Partial<ProjectDefaults>) : null;
  } catch {
    return null;
  }
}

export function normalizeProjectDefaults(value: Partial<ProjectDefaults>): ProjectDefaults {
  const effort = value.effort === "low" || value.effort === "high" ? value.effort : "medium";
  const approval_scope = ["once", "session", "project", "always"].includes(
    value.approval_scope ?? "",
  )
    ? (value.approval_scope as ApprovalDefault)
    : "once";
  const max_steps = Number.isInteger(value.max_steps)
    ? Math.min(50, Math.max(1, value.max_steps as number))
    : DEFAULT_PROJECT_DEFAULTS.max_steps;
  return {
    model: typeof value.model === "string" && value.model.trim() ? value.model.trim() : DEFAULT_PROJECT_DEFAULTS.model,
    effort,
    max_steps,
    approval_scope,
  };
}

export function ProjectDefaultsEditor({
  project,
  value,
  onChange,
}: {
  project: string;
  value: ProjectDefaults;
  onChange: (value: ProjectDefaults) => void;
}) {
  const update = <K extends keyof ProjectDefaults>(key: K, next: ProjectDefaults[K]) =>
    onChange({ ...value, [key]: next });

  return (
    <section className="project-defaults" aria-label="Project session defaults">
      <h3>New session defaults</h3>
      <p className="fine">Saved for this project. Each session keeps the values selected when it starts.</p>
      <label>
        Model
        <input aria-label="Default model" value={value.model} onChange={(event) => update("model", event.target.value)} />
      </label>
      <label>
        Reasoning effort
        <select aria-label="Default effort" value={value.effort} onChange={(event) => update("effort", event.target.value as ProjectDefaults["effort"])}>
          <option value="low">Low</option><option value="medium">Medium</option><option value="high">High</option>
        </select>
      </label>
      <label>
        Step cap
        <input aria-label="Default step cap" type="number" min={1} max={50} value={value.max_steps} onChange={(event) => update("max_steps", Number(event.target.value))} />
      </label>
      <label>
        Approval default
        <select aria-label="Default approval scope" value={value.approval_scope} onChange={(event) => update("approval_scope", event.target.value as ApprovalDefault)}>
          <option value="once">Allow once</option><option value="session">This session</option><option value="project">This project</option><option value="always">Always</option>
        </select>
      </label>
      <span className="fine">Project: {project}</span>
    </section>
  );
}

/** Load and persist a project's defaults while preventing a previous project's values flashing. */
export function useProjectDefaults(project: string | null) {
  const [value, setValue] = useState<ProjectDefaults>({ ...DEFAULT_PROJECT_DEFAULTS });
  useEffect(() => {
    if (project) setValue(loadProjectDefaults(project));
    else setValue({ ...DEFAULT_PROJECT_DEFAULTS });
  }, [project]);
  const change = (next: ProjectDefaults) => {
    const normalized = normalizeProjectDefaults(next);
    setValue(normalized);
    if (project) {
      try { saveProjectDefaults(project, normalized); } catch { /* storage may be unavailable */ }
    }
  };
  return [value, change] as const;
}
