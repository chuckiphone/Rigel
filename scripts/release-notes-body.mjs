#!/usr/bin/env node
/**
 * Rewrite relative Markdown document links for a GitHub release body.
 *
 * Release notes live in `docs/release-notes/`, so a link such as `../sso.md` is correct in the
 * repository and in GitHub's file view. The release page resolves it from the wrong base and
 * turns it into a 404, so publishing uses a temporary copy whose relative `.md` links point at
 * the same file under the release tag. The checked-in note stays portable and checkable.
 */
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import path from "node:path";

const NOTES_DIR = "docs/release-notes";

export function publishableNotes(source, { repository, tag } = {}) {
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository ?? "")) {
    throw new Error("publishableNotes needs a repository as owner/name");
  }
  if (!tag) throw new Error("publishableNotes needs a release tag");

  let rewritten = 0;
  const body = source.replace(
    /\]\(([^)\s]+?\.md(?:#[^)\s]*)?)(\s+"[^"]*")?\)/g,
    (match, target, title = "") => {
      if (/^(?:[a-z][a-z0-9+.-]*:|#)/i.test(target) || target.startsWith("/")) return match;
      const [file, fragment] = target.split("#");
      const resolved = path.posix.normalize(path.posix.join(NOTES_DIR, file));
      if (resolved === ".." || resolved.startsWith("../")) return match;
      rewritten += 1;
      const anchor = fragment ? `#${fragment}` : "";
      return `](https://github.com/${repository}/blob/${tag}/${resolved}${anchor}${title})`;
    },
  );

  return { body, rewritten };
}

function main() {
  const [notes, tag, output] = process.argv.slice(2);
  const repository = process.env.GITHUB_REPOSITORY;
  if (!notes || !tag || !output || !repository) {
    console.error(
      "usage: node scripts/release-notes-body.mjs <notes.md> <tag> <output.md> " +
        "(GITHUB_REPOSITORY is required)",
    );
    process.exit(2);
  }
  const { body, rewritten } = publishableNotes(readFileSync(notes, "utf8"), {
    repository,
    tag,
  });
  writeFileSync(output, body);
  console.log(
    `release notes: ${rewritten} relative doc link(s) anchored to ${tag} in ${output}`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
