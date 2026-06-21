// src/linkify.test.ts
import { describe, it, expect } from "vitest";
import type { ReactElement } from "react";
import { linkify } from "./linkify";

// Narrow a node to an anchor element for assertions.
function asAnchor(node: unknown): ReactElement<{ href: string; children: string }> {
  const el = node as ReactElement<{ href: string; children: string }>;
  expect(el.type).toBe("a");
  return el;
}

describe("linkify", () => {
  it("returns plain text unchanged when there is no url", () => {
    const out = linkify("hello world");
    expect(out).toHaveLength(1);
    expect(out[0]).toBe("hello world");
  });

  it("linkifies a lone url", () => {
    const out = linkify("https://example.com");
    expect(out).toHaveLength(1);
    const a = asAnchor(out[0]);
    expect(a.props.href).toBe("https://example.com");
    expect(a.props.children).toBe("https://example.com");
  });

  it("linkifies a url in the middle of a sentence", () => {
    const out = linkify("see https://example.com now");
    expect(out).toHaveLength(3);
    expect(out[0]).toBe("see ");
    expect(asAnchor(out[1]).props.href).toBe("https://example.com");
    expect(out[2]).toBe(" now");
  });

  it("linkifies multiple urls", () => {
    const out = linkify("a http://one.com b https://two.com c");
    const hrefs = out
      .filter((n) => typeof n !== "string")
      .map((n) => asAnchor(n).props.href);
    expect(hrefs).toEqual(["http://one.com", "https://two.com"]);
    expect(out[0]).toBe("a ");
    expect(out[out.length - 1]).toBe(" c");
  });

  it("strips trailing sentence punctuation from the link", () => {
    const out = linkify("visit https://example.com.");
    expect(out).toHaveLength(3);
    expect(asAnchor(out[1]).props.href).toBe("https://example.com");
    expect(out[2]).toBe(".");
  });

  it("keeps a balanced closing paren inside the url", () => {
    const url = "https://en.wikipedia.org/wiki/Foo_(bar)";
    const out = linkify(url);
    expect(out).toHaveLength(1);
    expect(asAnchor(out[0]).props.href).toBe(url);
  });

  it("returns an empty array for an empty string", () => {
    expect(linkify("")).toEqual([]);
  });
});
