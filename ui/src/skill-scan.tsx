import type { SkillInstallReport, SkillScan } from "./api";

/** One line for a notice or a CLI-style receipt. */
export function scanReceiptText(scan: SkillScan, blocked = 0): string {
  return `SkillSpector ${scan.recommendation} ${scan.score}/100${
    blocked ? ` · ${blocked} blocked` : ""
  }`;
}

/** The install notice, including the scan that admitted the exact bytes written. */
export function installReceiptText(report: SkillInstallReport): string {
  return `${report.name} installed in ${report.paths.length} skill store${
    report.paths.length === 1 ? "" : "s"
  } · ${scanReceiptText(report.scan)}.${
    report.replaced ? " The previous copy was replaced." : ""
  }`;
}

/**
 * The Settings receipt. Keeping this as a component (instead of another interpolated string)
 * gives the renderer test a real React tree to render without starting Tauri.
 */
export function SkillScanReceipt({
  scan,
  blocked = 0,
}: {
  scan: SkillScan;
  blocked?: number;
}) {
  return (
    <span className="skill-scan-receipt">
      SkillSpector {scan.recommendation} {scan.score}/100
      {blocked ? ` · ${blocked} blocked` : ""}
    </span>
  );
}
