// src/linkify.tsx
import { openUrl } from "@tauri-apps/plugin-opener";

const URL_RE = /(https?:\/\/[^\s]+)/g;
const TRAILING = /[.,;:!?'"]+$/;

// Strip trailing sentence punctuation and an unbalanced closing bracket from a
// matched URL. Returns [cleanUrl, strippedSuffix]. Best-effort, not a full URL
// parser: a trailing ")" is kept when the URL has an unbalanced "(" so that
// links like .../Foo_(bar) survive.
function splitTrailing(url: string): [string, string] {
  let suffix = "";
  let u = url;

  const punct = u.match(TRAILING);
  if (punct) {
    suffix = punct[0] + suffix;
    u = u.slice(0, -punct[0].length);
  }

  const last = u[u.length - 1];
  const opener = last === ")" ? "(" : last === "]" ? "[" : last === "}" ? "{" : "";
  if (opener) {
    const opens = u.split(opener).length - 1;
    const closes = u.split(last).length - 1;
    if (closes > opens) {
      suffix = last + suffix;
      u = u.slice(0, -1);
    }
  }

  return [u, suffix];
}

export function linkify(text: string): React.ReactNode[] {
  const nodes: React.ReactNode[] = [];
  let lastIndex = 0;
  let key = 0;

  for (const match of text.matchAll(URL_RE)) {
    const raw = match[0];
    const start = match.index ?? 0;

    if (start > lastIndex) nodes.push(text.slice(lastIndex, start));

    const [url, suffix] = splitTrailing(raw);
    nodes.push(
      <a
        key={key++}
        href={url}
        onClick={(e) => {
          e.preventDefault();
          openUrl(url).catch((err) => console.warn("openUrl failed", err));
        }}
      >
        {url}
      </a>,
    );
    if (suffix) nodes.push(suffix);

    lastIndex = start + raw.length;
  }

  if (lastIndex < text.length) nodes.push(text.slice(lastIndex));

  return nodes;
}
