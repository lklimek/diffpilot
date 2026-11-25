import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppNav } from "./AppNav";
import { ShellTopBar } from "./ShellTopBar";

const HREFS = { "my-work": "/", review: "/review", watching: "/watching", settings: "/settings" };

describe("AppNav", () => {
  beforeEach(() => window.localStorage.clear());

  it("marks the current section and shows the My work count", () => {
    render(<AppNav current="my-work" hrefs={HREFS} myWorkCount={14} />);
    const myWork = screen.getByRole("link", { name: /My work/ });
    expect(myWork).toHaveAttribute("aria-current", "page");
    expect(myWork).toHaveAttribute("href", "/");
    expect(myWork).toHaveTextContent("14");
    expect(screen.getByRole("link", { name: "Review a PR" })).not.toHaveAttribute("aria-current");
  });

  it("navigates in the app on a plain click", () => {
    const onNavigate = vi.fn();
    render(<AppNav current="my-work" hrefs={HREFS} myWorkCount={null} onNavigate={onNavigate} />);
    fireEvent.click(screen.getByRole("link", { name: "Settings" }));
    expect(onNavigate).toHaveBeenCalledWith("/settings");
  });

  it("collapses to icons with accessible names and remembers it", async () => {
    const { unmount } = render(<AppNav current="review" hrefs={HREFS} myWorkCount={3} />);
    await userEvent.click(screen.getByRole("button", { name: "Collapse navigation" }));
    expect(screen.getByRole("link", { name: "Review a PR" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("button", { name: "Expand navigation" })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
    unmount();
    render(<AppNav current="review" hrefs={HREFS} myWorkCount={3} />);
    expect(screen.getByRole("button", { name: "Expand navigation" })).toBeInTheDocument();
  });

  it("explains each entry in a tooltip on focus", () => {
    render(<AppNav current={null} hrefs={HREFS} myWorkCount={null} />);
    fireEvent.focus(screen.getByRole("link", { name: "Review a PR" }));
    expect(screen.getByRole("tooltip")).toHaveTextContent("Open a pull request by URL");
  });
});

describe("ShellTopBar", () => {
  it("starts with a skip link and holds the page actions, help and the user", async () => {
    const onHelp = vi.fn();
    render(
      <ShellTopBar
        crumb="My work"
        homeHref="/"
        userMenu={{
          login: "alice",
          settingsHref: "/settings",
          usageHref: "/settings#llm-usage",
          now: new Date(),
          activity: null,
          onHelp,
          onSignOut: vi.fn(),
        }}
      >
        <button type="button">Refresh</button>
      </ShellTopBar>,
    );
    const links = screen.getAllByRole("link");
    expect(links[0]).toHaveTextContent("Skip to content");
    expect(links[0]).toHaveAttribute("href", "#main");
    expect(screen.queryByRole("button", { name: "Keyboard shortcuts" })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "alice" }));
    expect(screen.getByRole("link", { name: "Settings" })).toHaveAttribute("href", "/settings");
    expect(screen.getByRole("button", { name: "Refresh" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Keyboard shortcuts" }));
    expect(onHelp).toHaveBeenCalledOnce();
  });

  it("has no user menu when signed out", () => {
    render(<ShellTopBar homeHref="/" />);
    expect(screen.queryByRole("button", { name: "alice" })).toBeNull();
  });
});

it("pluralizes the mention count and preserves the section description", () => {
  render(<AppNav current="my-work" hrefs={HREFS} myWorkCount={1} />);
  const link = screen.getByRole("link", { name: "My work, 1 unanswered mention from people" });
  fireEvent.focus(link);
  expect(screen.getByRole("tooltip")).toHaveTextContent(
    "Your PRs, issues and TODOs, ranked; throughput charts",
  );
  expect(screen.getByRole("tooltip")).toHaveTextContent("1 unanswered mention from people");
});
