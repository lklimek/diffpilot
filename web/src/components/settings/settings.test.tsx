import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import type { RepoSettings } from "../../api/generated/types";
import { AppInstallationsCard } from "./AppInstallationsCard";
import { LanguageCard } from "./LanguageCard";
import { LlmStatusCard } from "./LlmStatusCard";
import { RepoList } from "./RepoList";
import { RepoSettingsCard } from "./RepoSettingsCard";

const at = new Date(2026, 10, 17, 8, 5).toISOString();

describe("LlmStatusCard (brief §4.4: outcome of the most recent call, no on-demand check)", () => {
  it("working: status with tooltip and the time of the call", () => {
    render(<LlmStatusCard status={{ at, status: { state: "working" } }} />);
    const status = screen.getByText("working");
    expect(status).toHaveAccessibleDescription(/most recent LLM call succeeded/);
    expect(screen.getByRole("region", { name: "LLM status" })).toHaveTextContent("17 Nov 2026, 08:05");
    expect(screen.queryByRole("button")).toBeNull();
    expect(document.body).toHaveTextContent("never reach the browser");
  });

  it("not working: the plain error message and the time", () => {
    render(
      <LlmStatusCard
        status={{ at, status: { state: "not_working", message: "Codex usage limit reached." } }}
      />,
    );
    expect(screen.getByText("not working")).toHaveAccessibleDescription(/most recent LLM call failed/);
    expect(screen.getByText("Codex usage limit reached.")).toBeVisible();
    expect(screen.getByRole("region", { name: "LLM status" })).toHaveTextContent("08:05");
  });

  it("no call yet: says the status is not known instead of a value", () => {
    render(<LlmStatusCard status={null} />);
    const region = screen.getByRole("region", { name: "LLM status" });
    expect(region).toHaveTextContent("not known yet");
    expect(within(region).queryByText("working")).toBeNull();
    expectTrustWording(region.textContent ?? "");
  });
});

describe("LanguageCard", () => {
  it("shows the language and reports a change", async () => {
    const onChange = vi.fn();
    render(<LanguageCard language="en" busy={false} onChange={onChange} />);
    const select = screen.getByLabelText(/written in/);
    expect(select).toHaveValue("en");
    await userEvent.selectOptions(select, "pl");
    expect(onChange).toHaveBeenCalledWith("pl");
    expect(document.body).toHaveTextContent("The interface stays English.");
  });

  it("is disabled while saving", () => {
    render(<LanguageCard language="pl" busy onChange={() => {}} />);
    expect(screen.getByLabelText(/written in/)).toBeDisabled();
  });
});

describe("AppInstallationsCard", () => {
  it("lists installations with their repository scope", () => {
    render(
      <AppInstallationsCard
        installations={[
          { account: "acme", all_repositories: false },
          { account: "beta", all_repositories: true },
        ]}
        signInHref="/auth/github/login"
      />,
    );
    const region = screen.getByRole("region", { name: "GitHub App" });
    expect(region).toHaveTextContent("acme: selected repositories");
    expect(region).toHaveTextContent("beta: all repositories");
    expect(within(region).getByRole("link", { name: /Manage on GitHub/ })).toHaveAttribute(
      "target",
      "_blank",
    );
  });

  it("not checked: says so without inventing a cause, offers signing in again", () => {
    render(<AppInstallationsCard installations={null} signInHref="/auth/github/login" />);
    const region = screen.getByRole("region", { name: "GitHub App" });
    expect(region).toHaveTextContent("GitHub App installations: not checked. Signing in again may help.");
    expect(region).not.toHaveTextContent("not installed");
    expect(region).not.toHaveTextContent("could not be reached");
    expect(within(region).getByRole("link", { name: "Sign in with GitHub" })).toHaveAttribute(
      "href",
      "/auth/github/login",
    );
  });

  it("not installed anywhere: says what does not work", () => {
    render(<AppInstallationsCard installations={[]} signInHref="/auth/github/login" />);
    expect(screen.getByRole("region", { name: "GitHub App" })).toHaveTextContent(
      "not installed on any account",
    );
  });
});

const repoSettings: RepoSettings = {
  repo: { owner: "acme", name: "node-client" },
  rules: [
    { kind: "path_glob", pattern: "tests/**", category: "tests" },
    { kind: "line_regex", pattern: "^\\s*//", category: "docs_comments" },
  ],
  related_repos: [{ owner: "acme", name: "node-sdk" }],
  project_profile: { sources: ["repository_file", "memcan"], memcan: "checked" },
};

describe("RepoSettingsCard (read-only)", () => {
  it("shows rules, related repositories and the profile sources", () => {
    render(<RepoSettingsCard settings={repoSettings} />);
    const region = screen.getByRole("region", { name: /acme\/node-client/ });
    const rows = within(region)
      .getAllByRole("row")
      .slice(1)
      .map((r) => r.textContent);
    expect(rows).toEqual([
      "tests/**path globMatches the file path.Tests",
      "^\\s*//line regexMatches a line with a regular expression.Docs & comments",
    ]);
    for (const name of ["Rule", "Kind", "Section"]) {
      const header = within(region).getByRole("columnheader", { name });
      expect(within(header).getByText(name)).toHaveAccessibleDescription(/.+/);
    }
    expect(within(region).getByText("acme/node-sdk")).toBeVisible();
    expect(within(region).queryAllByRole("link")).toEqual([]);
    expect(region).toHaveTextContent(".claudius/project-profile.md on the base ref");
    expect(region).toHaveTextContent("MemCan");
    expect(region).toHaveTextContent("the repository file wins");
  });

  it("states the absence of related repositories and of a profile", () => {
    render(
      <RepoSettingsCard
        settings={{
          ...repoSettings,
          rules: [],
          related_repos: [],
          project_profile: { sources: [], memcan: "checked" },
        }}
      />,
    );
    const region = screen.getByRole("region", { name: /acme\/node-client/ });
    expect(region).toHaveTextContent("No built-in rules");
    expect(region).toHaveTextContent("None detected");
    expect(region).toHaveTextContent("No project profile");
    expectTrustWording(region.textContent ?? "");
  });
});

describe("RepoSettingsCard profile with MemCan not checked", () => {
  it.each([
    [[], "No .claudius/project-profile.md on the base ref. Project profile in MemCan: not checked."],
    [
      ["repository_file"],
      ".claudius/project-profile.md on the base ref. Project profile in MemCan: not checked.",
    ],
  ] as const)("never implies no MemCan profile exists (sources %j)", (sources, text) => {
    render(
      <RepoSettingsCard
        settings={{ ...repoSettings, project_profile: { sources: [...sources], memcan: "not_checked" } }}
      />,
    );
    const region = screen.getByRole("region", { name: /acme\/node-client/ });
    expect(region).toHaveTextContent(text);
    expect(region).not.toHaveTextContent(/nor a MemCan profile/);
  });
});

describe("RepoList", () => {
  it("marks the selected repository and reports a choice", async () => {
    const onSelect = vi.fn();
    const repos = [
      { owner: "acme", name: "node-client" },
      { owner: "acme", name: "wallet-ui" },
    ];
    render(<RepoList repos={repos} selected={repos[0]!} onSelect={onSelect} />);
    expect(screen.getByRole("button", { name: "acme/node-client" })).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(screen.getByRole("button", { name: "acme/wallet-ui" }));
    expect(onSelect).toHaveBeenCalledWith(repos[1]);
  });
});

it("names lexer, header and binary detectors with their actual checks", () => {
  render(
    <RepoSettingsCard
      settings={{
        repo: { owner: "acme", name: "node" },
        related_repos: [],
        project_profile: { sources: [], memcan: "not_checked" },
        rules: [
          { kind: "lexer", pattern: "Rust comments", category: "docs_comments" },
          { kind: "header", pattern: "@generated", category: "generated_mechanical" },
          { kind: "binary", pattern: "binary", category: "generated_mechanical" },
        ],
      }}
    />,
  );
  expect(screen.getByText("lexer")).toHaveAccessibleDescription(/Rust compiler lexer/);
  expect(screen.getByText("generated-file header")).toHaveAccessibleDescription(/generated-file header/);
  expect(screen.getByText("binary content")).toHaveAccessibleDescription(/Git identifies binary/);
});
