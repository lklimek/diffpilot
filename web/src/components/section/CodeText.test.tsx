import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { CodeText } from "./CodeText";

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
