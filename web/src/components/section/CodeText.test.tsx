import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { CodeText } from "./CodeText";
import { PathLinks, pathMatcher } from "./pathLinks";

describe("restricted code notation", () => {
  it.each([
    "before `unfinished",
    "```js\nunfinished `code`",
    "``double``",
    "**bold** [link](javascript:alert(1)) <img src=x>",
  ])("keeps unsupported or incomplete text literal: %s", (text) => {
    const { container } = render(<CodeText text={text} />);
    expect(container.textContent).toBe(text);
    expect(container.querySelector("a,img,script,strong,pre")).toBeNull();
  });
  it("renders several inline spans and escapes fenced HTML", () => {
    const { container } = render(
      <CodeText text={'`one` / `two`\n```html\n<a href="javascript:alert(1)">x</a>\n```'} />,
    );
    expect([...container.querySelectorAll("code")].map((c) => c.textContent)).toEqual([
      "one",
      "two",
      '<a href="javascript:alert(1)">x</a>\n',
    ]);
    expect(container.querySelector("a")).toBeNull();
  });
});

it("links file mentions in prose and backticks, leaving URLs and code fences literal", () => {
  const match = pathMatcher(["src/lib.rs"], { owner: "acme", name: "node" });
  const onNavigate = vi.fn();
  const href = "/analyses/example/files#lines?file=src%2Flib.rs&side=head";
  const { container } = render(
    <PathLinks.Provider
      value={{
        resolve: (mention) => (match(mention) ? { href, tip: "Open src/lib.rs in Files" } : undefined),
        onNavigate,
      }}
    >
      <CodeText
        text={`See src/lib.rs, then \`src/lib.rs:2\`. https://example.test/src/lib.rs
\`\`\`text
src/lib.rs
\`\`\``}
      />
    </PathLinks.Provider>,
  );
  const [plain, inline] = screen.getAllByRole("link");
  expect(plain).toHaveAttribute("href", href);
  expect(inline?.closest("code")).toBeTruthy();
  expect(container).toHaveTextContent("https://example.test/src/lib.rs");
  expect(container.querySelector("pre a")).toBeNull();
  fireEvent.click(inline!);
  expect(onNavigate).toHaveBeenCalledWith(href);
});
