import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CodeText } from "./CodeText";
import { PathLinks, pathMatcher } from "./pathLinks";

const match = pathMatcher(["src/lib.rs", "Cargo.toml"], { owner: "acme", name: "node" });
const show = (text: string, onNavigate = vi.fn()) =>
  render(
    <PathLinks.Provider
      value={{
        resolve: (m) => (match(m) ? { href: `/x#${encodeURIComponent(m)}`, tip: "t" } : undefined),
        onNavigate,
      }}
    >
      <CodeText text={text} />
    </PathLinks.Provider>,
  );

afterEach(() => vi.restoreAllMocks());

describe("prose punctuation after a mention", () => {
  it.each([
    "In src/lib.rs: the retry loop",
    "Did you check src/lib.rs?",
    "See src/lib.rs:12: the call",
    "(src/lib.rs)",
    "src/lib.rs, then Cargo.toml.",
  ])("links the mention in %j", (text) => {
    const { container } = show(text);
    expect(container.textContent).toBe(text);
    expect(screen.queryAllByRole("link").map((a) => a.textContent)).toContain(
      text.includes("src/lib.rs:12") ? "src/lib.rs:12" : "src/lib.rs",
    );
  });
});

it("never links a URL or a scheme-like token", () => {
  show(
    "javascript:alert(1) src/lib.rs@evil https://evil.test/src/lib.rs? file:src/lib.rs: src/lib.rs?next=src/lib.rs",
  );
  expect(screen.queryAllByRole("link")).toHaveLength(0);
});

it("renders several mentions without React key warnings", () => {
  const error = vi.spyOn(console, "error").mockImplementation(() => {});
  show("src/lib.rs and Cargo.toml and `src/lib.rs:3` and src/lib.rs");
  expect(screen.getAllByRole("link")).toHaveLength(4);
  expect(error).not.toHaveBeenCalled();
});

it("routes only plain primary clicks through the app", () => {
  const onNavigate = vi.fn();
  show("src/lib.rs", onNavigate);
  const link = screen.getByRole("link");
  for (const modifier of ["ctrlKey", "metaKey", "shiftKey", "altKey"]) {
    const event = new MouseEvent("click", { bubbles: true, cancelable: true, [modifier]: true });
    // Cancel jsdom's default navigation after the React handler runs.
    window.addEventListener("click", (click) => click.preventDefault(), { once: true });
    link.dispatchEvent(event);
  }
  expect(onNavigate).not.toHaveBeenCalled();
  link.click();
  expect(onNavigate).toHaveBeenCalledExactlyOnceWith("/x#src%2Flib.rs");
});
