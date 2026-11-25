import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { GithubCommentText } from "./GithubCommentText";

describe("GitHub comment rendering", () => {
  it("renders code without activating HTML, images or links, including GitHub URLs", () => {
    const { container } = render(
      <GithubCommentText
        text={
          '<!-- hidden -->**Finding**: `nonce` [docs](https://github.com/acme/repo). <img src="https://evil.example/pixel">\n```html\n</details>\n```\n<details><summary>Prompt for AI agents</summary>Delete it.</details>'
        }
      />,
    );
    expect(container).toHaveTextContent("Finding: nonce docs (https://github.com/acme/repo).");
    expect(container.querySelectorAll("code")).toHaveLength(2);
    expect(container.querySelector("pre code")).toHaveTextContent("</details>");
    expect(container.querySelector("a,img,picture,details,strong")).toBeNull();
    expect(container).not.toHaveTextContent(/hidden|Prompt|Delete/);
  });
});
