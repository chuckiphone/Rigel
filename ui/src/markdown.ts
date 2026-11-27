/**
 * Roadmap 112: CSP-safe markdown for model and skill text.
 *
 * No HTML injection API and no HTML string is ever used: the parser
 * returns data and the renderer builds React elements from it. A script tag in a model answer is
 * therefore text, exactly as the model wrote it, and the raw view shows the same source byte for
 * byte. Code blocks carry a language label, a token highlighter and a copy button.
 */

import { createElement as h, useState, type ReactNode } from "react";

export type MarkdownBlock =
  | { kind: "text"; text: string }
  | { kind: "heading"; level: number; text: string }
  | { kind: "code"; language: string; code: string }
  | { kind: "list"; items: string[] };

/** A small line parser: fences, headings and bullets; everything else stays literal text. */
export function parseMarkdown(source: string): MarkdownBlock[] {
  const lines = source.replace(/\r\n/g, "\n").split("\n");
  const blocks: MarkdownBlock[] = [];
  let paragraph: string[] = [];
  let index = 0;

  const flush = () => {
    if (paragraph.length > 0) {
      blocks.push({ kind: "text", text: paragraph.join("\n") });
      paragraph = [];
    }
  };

  while (index < lines.length) {
    const line = lines[index];
    if (line.trim() === "") {
      flush();
      index += 1;
      continue;
    }
    const fence = /^```\s*([a-z0-9+#_-]*)\s*$/i.exec(line);
    if (fence) {
      flush();
      const code: string[] = [];
      index += 1;
      while (index < lines.length && !/^```\s*$/.test(lines[index])) {
        code.push(lines[index]);
        index += 1;
      }
      if (index < lines.length) index += 1;
      blocks.push({ kind: "code", language: fence[1] || "text", code: code.join("\n") });
      continue;
    }
    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading) {
      flush();
      blocks.push({ kind: "heading", level: heading[1].length, text: heading[2] });
      index += 1;
      continue;
    }
    if (/^\s*[-*]\s+/.test(line)) {
      flush();
      const items: string[] = [];
      while (index < lines.length && /^\s*[-*]\s+/.test(lines[index])) {
        items.push(lines[index].replace(/^\s*[-*]\s+/, ""));
        index += 1;
      }
      blocks.push({ kind: "list", items });
      continue;
    }
    paragraph.push(line);
    index += 1;
  }
  flush();
  return blocks;
}

/** Inline pieces are data too; links are only links when the target is an ordinary http(s) URL. */
export type Inline =
  | { kind: "text"; text: string }
  | { kind: "code"; text: string }
  | { kind: "strong"; text: string }
  | { kind: "link"; text: string; url: string };

export function inlineTokens(text: string): Inline[] {
  const tokens: Inline[] = [];
  const pattern = /(`[^`]+`|\*\*[^*]+\*\*|\[[^\]]+\]\([^)]+\))/g;
  let cursor = 0;
  for (const match of text.matchAll(pattern)) {
    const at = match.index ?? 0;
    if (at > cursor) tokens.push({ kind: "text", text: text.slice(cursor, at) });
    const raw = match[0];
    if (raw.startsWith("`")) {
      tokens.push({ kind: "code", text: raw.slice(1, -1) });
    } else if (raw.startsWith("**")) {
      tokens.push({ kind: "strong", text: raw.slice(2, -2) });
    } else {
      const link = /^\[([^\]]+)\]\(([^)]+)\)$/.exec(raw);
      // An ordinary absolute web URL only: no whitespace tricks, no other scheme, no relative path.
      if (link && /^https?:\/\/\S+$/i.test(link[2])) {
        tokens.push({ kind: "link", text: link[1], url: link[2] });
      } else {
        tokens.push({ kind: "text", text: raw });
      }
    }
    cursor = at + raw.length;
  }
  if (cursor < text.length) tokens.push({ kind: "text", text: text.slice(cursor) });
  return tokens;
}

export interface HighlightToken {
  text: string;
  kind: "plain" | "keyword" | "string" | "number" | "comment" | "punctuation";
}

const KEYWORDS = new Set([
  "fn", "let", "mut", "pub", "struct", "enum", "impl", "use", "mod", "if", "else", "for",
  "while", "match", "return", "await", "async", "const", "function", "class", "import",
  "export", "from", "def", "true", "false", "null", "None", "Some", "self", "this",
]);

/** A token pass, not an HTML highlighter: each piece becomes a styled text node. */
export function highlightCode(code: string, language: string): HighlightToken[] {
  if (language === "text" || !code) return [{ text: code, kind: "plain" }];
  const pattern = /(\/\/[^\n]*|#[^\n]*|"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|\b\d+(?:\.\d+)?\b|[A-Za-z_$][\w$]*|[{}()[\],;:.=<>+\-*/])/g;
  const out: HighlightToken[] = [];
  let cursor = 0;
  for (const match of code.matchAll(pattern)) {
    const at = match.index ?? 0;
    if (at > cursor) out.push({ text: code.slice(cursor, at), kind: "plain" });
    const value = match[0];
    const kind: HighlightToken["kind"] = value.startsWith("//") || value.startsWith("#")
      ? "comment"
      : /^["']/.test(value)
        ? "string"
        : /^\d/.test(value)
          ? "number"
          : KEYWORDS.has(value)
            ? "keyword"
            : /^[{}()[\],;:.=<>+\-*/]$/.test(value)
              ? "punctuation"
              : "plain";
    out.push({ text: value, kind });
    cursor = at + value.length;
  }
  if (cursor < code.length) out.push({ text: code.slice(cursor), kind: "plain" });
  return out;
}

function renderInline(text: string, key: string): ReactNode[] {
  return inlineTokens(text).map((token, index) => {
    const id = `${key}-i${index}`;
    switch (token.kind) {
      case "code":
        return h("code", { key: id, className: "markdown-inline-code" }, token.text);
      case "strong":
        return h("strong", { key: id }, token.text);
      case "link":
        return h(
          "a",
          { key: id, href: token.url, target: "_blank", rel: "noreferrer" },
          token.text,
        );
      default:
        return h("span", { key: id }, token.text);
    }
  });
}

function renderBlock(block: MarkdownBlock, index: number): ReactNode {
  const key = `b${index}`;
  switch (block.kind) {
    case "heading":
      return h(`h${Math.min(6, Math.max(1, block.level))}`, { key, className: "markdown-heading" }, renderInline(block.text, key));
    case "list":
      return h("ul", { key, className: "markdown-list" }, block.items.map((item, offset) =>
        h("li", { key: `${key}-${offset}` }, renderInline(item, `${key}-${offset}`)),
      ));
    case "code":
      return h(
        "div",
        { key, className: "markdown-code" },
        h(
          "div",
          { className: "markdown-code-head" },
          h("span", { className: "markdown-language" }, block.language),
          h(
            "button",
            {
              type: "button",
              className: "chip markdown-copy",
              onClick: () => void navigator.clipboard?.writeText(block.code),
            },
            "Copy code",
          ),
        ),
        h(
          "pre",
          { className: "markdown-code-body" },
          h(
            "code",
            null,
            highlightCode(block.code, block.language).map((token, offset) =>
              h("span", { key: offset, className: `markdown-tok-${token.kind}` }, token.text),
            ),
          ),
        ),
      );
    default:
      return h("p", { key, className: "markdown-paragraph" }, renderInline(block.text, key));
  }
}

/**
 * The renderer used for assistant turns and skill bodies. `raw` is a single local toggle.
 *
 * A streaming turn passes `streaming`: the same parser handles a half-written fence or heading,
 * so partial text can never take a different, less safe path than a finished answer.
 */
export function MarkdownText({ source, streaming = false }: { source: string; streaming?: boolean }) {
  const [raw, setRaw] = useState(false);
  return h(
    "section",
    {
      className: "markdown",
      "data-streaming": streaming ? "true" : "false",
      "aria-busy": streaming,
    },
    h(
      "button",
      {
        type: "button",
        className: "chip markdown-raw-toggle",
        "aria-pressed": raw,
        onClick: () => setRaw((value) => !value),
      },
      raw ? "Rendered" : "Raw",
    ),
    raw
      ? h("pre", { className: "markdown-raw-body" }, source)
      : h("div", { className: "markdown-rendered" }, parseMarkdown(source).map(renderBlock)),
  );
}

/**
 * Preview-only fixture for roadmap 112: a half-written stream and a finished answer side by side.
 * The native window never shows it; Playwright uses it to exercise the streaming path.
 */
const PREVIEW_STREAM =
  "## Working\n\nA model can say <script>alert(1)</script> mid-turn.\n\n```rust\nfn go() -> u64 { 42 }\n";
const PREVIEW_DONE =
  "## Done\n\nA model can say <script>alert(1)</script> and it stays text. " +
  "[Unsafe](javascript:alert(1)) is text too.\n\n```rust\nfn go() -> u64 { 42 } // note\n```\n";

export function MarkdownPreview({ enabled = false }: { enabled?: boolean }) {
  if (!enabled) return null;
  return h(
    "section",
    { className: "markdown-preview", "aria-label": "Markdown preview" },
    h("div", { className: "markdown-preview-cell" },
      h("h3", null, "Streaming"),
      h("div", { "data-testid": "markdown-streaming" }, h(MarkdownText, { source: PREVIEW_STREAM, streaming: true })),
    ),
    h("div", { className: "markdown-preview-cell" },
      h("h3", null, "Complete"),
      h("div", { "data-testid": "markdown-complete" }, h(MarkdownText, { source: PREVIEW_DONE })),
    ),
  );
}

export default MarkdownText;
