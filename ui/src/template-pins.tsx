/**
 * Roadmap 99: a template's skill pins travel with their body hashes. The row shows how many are
 * pinned and which of them no longer match the current session, and starting from a template with
 * moved hashes asks before restoring the older bodies.
 */

import type { SkillPin, TemplateSkill } from "./api";

/** Skill ids whose pinned hash differs from the hash the current session holds. */
export function changedTemplatePins(skills: TemplateSkill[], pins: SkillPin[]): string[] {
  const current = new Map(pins.map((pin) => [pin.skill_id, pin.body_sha]));
  return skills
    .filter((skill) => current.get(skill.skill_id) !== skill.body_sha)
    .map((skill) => skill.skill_id);
}

export function TemplatePinBadge({
  skills,
  changed,
}: {
  skills: number;
  changed: string[];
}) {
  if (skills === 0) return null;
  return (
    <span
      className={`template-pins${changed.length > 0 ? " changed" : ""}`}
      title={
        changed.length > 0
          ? `Pinned hash no longer matches: ${changed.join(", ")}`
          : "Every skill is pinned to its recorded body hash"
      }
    >
      {" · "}
      {skills} pinned{changed.length > 0 ? ` · ${changed.length} changed` : ""}
    </span>
  );
}
