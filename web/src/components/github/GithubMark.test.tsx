import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { OpenExternal } from "../dashboard/common/OpenExternal";
import { ITEMS } from "../dashboard/test-fixtures";
import { ItemAction } from "../dashboard/work-item/ItemAction";
import { GithubLink } from "../inspector/text";
import { GithubMark } from "./GithubMark";

const GITHUB = "https://github.com/acme/node-client/pull/482";
const githubIcon = (el: HTMLElement) => el.querySelector('svg[data-icon="github"]');

describe("GitHub link mark", () => {
  it("a GitHub text link ends with the GitHub mark, not ↗, and is named as opening on GitHub in a new tab", () => {
    render(
      <GithubLink url={GITHUB} tip="Open these lines on GitHub (new tab)">
        L12
      </GithubLink>,
    );
    const link = screen.getByRole("link", { name: "L12 (opens on GitHub in a new tab)" });
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAccessibleDescription("Open these lines on GitHub (new tab)");
    expect(githubIcon(link)).toHaveAttribute("aria-hidden", "true");
    // GitHub logo guidelines: the mark is not recoloured with the link; it stays the neutral muted grey.
    expect(githubIcon(link)).toHaveClass("text-muted");
    expect(link).not.toHaveTextContent("↗");
  });

  it("the mark alone is hidden from the visible text and names the destination for screen readers", () => {
    render(
      <a href={GITHUB}>
        #482
        <GithubMark />
      </a>,
    );
    expect(screen.getByRole("link", { name: "#482 (opens on GitHub in a new tab)" })).toBeInTheDocument();
  });

  it("an icon link to GitHub shows the GitHub mark and says new tab", () => {
    render(<OpenExternal href={GITHUB} label="Open PLAT-1 on GitHub" />);
    const link = screen.getByRole("link", { name: "Open PLAT-1 on GitHub (new tab)" });
    expect(link).toHaveAccessibleDescription("Open PLAT-1 on GitHub (new tab)");
    expect(githubIcon(link)).toHaveClass("text-muted");
  });

  it("an icon link elsewhere (MemCan) keeps the external arrow", () => {
    render(<OpenExternal href="http://memcan.lan/ui/tasks/1" label="Open TODO in MemCan" />);
    const link = screen.getByRole("link", { name: "Open TODO in MemCan (new tab)" });
    expect(githubIcon(link)).toBeNull();
    expect(link.querySelector("svg")).not.toBeNull();
  });

  it("an issue's Open action shows the GitHub mark; a TODO's keeps the arrow", () => {
    const issue = { ...ITEMS.find((i) => i.kind === "issue")!, url: "https://github.com/acme/x/issues/7" };
    const todo = { ...ITEMS.find((i) => i.kind === "todo")!, url: "http://memcan.lan/ui/tasks/1" };
    render(<ItemAction item={issue} />);
    const link = screen.getByRole("link", { name: "Open issue (opens on GitHub in a new tab)" });
    expect(githubIcon(link)).not.toBeNull();
    render(<ItemAction item={todo} />);
    const memcan = screen.getByRole("link", { name: /^Open TODO/ });
    expect(githubIcon(memcan)).toBeNull();
    expect(memcan.querySelector("svg")).not.toBeNull();
  });
});
