import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import type { RepoSettings } from "../../api/generated/types";
import { AppInstallationsCard } from "./AppInstallationsCard";
import { LanguageCard } from "./LanguageCard";
import { UsageLimitsSection } from "./UsageLimitsSection";
import { RepoSettingsCard } from "./RepoSettingsCard";

const at = new Date(2026, 10, 17, 8, 5).toISOString();

describe("Last LLM call", () => {
  it("shows the time and outcome of a successful call", () => {
    render(
      <UsageLimitsSection
        now={new Date(at)}
        onReset={vi.fn()}
        lastCall={{ at, status: { state: "working" } }}
      />,
    );
    expect(screen.getByRole("region", { name: "Last LLM call" })).toHaveTextContent(
      "Succeeded · 17 Nov 2026, 08:05",
    );
  });

  it("shows the time and plain error message of a failed call", () => {
    render(
      <UsageLimitsSection
        now={new Date(at)}
        onReset={vi.fn()}
        lastCall={{ at, status: { state: "not_working", message: "Codex usage limit reached." } }}
      />,
    );
    const call = screen.getByRole("region", { name: "Last LLM call" });
    expect(call).toHaveTextContent("Failed · 17 Nov 2026, 08:05");
    expect(call).toHaveTextContent("Codex usage limit reached.");
  });

  it("says when no LLM call has been made", () => {
    render(<UsageLimitsSection now={new Date(at)} onReset={vi.fn()} lastCall={null} />);
    const call = screen.getByRole("region", { name: "Last LLM call" });
    expect(call).toHaveTextContent("No LLM call yet.");
    expectTrustWording(call.textContent ?? "");
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

  it("offers English, Polish and Russian", async () => {
    const onChange = vi.fn();
    render(<LanguageCard language="pl" busy={false} onChange={onChange} />);
    const select = screen.getByLabelText(/written in/);
    expect(Array.from((select as HTMLSelectElement).options, (o) => [o.value, o.text])).toEqual([
      ["en", "English"],
      ["pl", "Polski"],
      ["ru", "Русский"],
    ]);
    await userEvent.selectOptions(select, "ru");
    expect(onChange).toHaveBeenCalledWith("ru");
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
  it("shows rules, related repositories and the profile sources", async () => {
    render(<RepoSettingsCard settings={repoSettings} />);
    const region = screen.getByRole("region", { name: /acme\/node-client/ });
    await userEvent.click(screen.getByText("Show the 2 built-in rules"));
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
    // Read-only: no links besides the ? help.
    expect(
      within(region)
        .queryAllByRole("link")
        .filter((link) => !link.textContent?.includes("Help: ")),
    ).toEqual([]);
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

it("shows today's and seven-day provider totals separately", () => {
  const usage = {
    input_tokens: 3_400_000,
    cached_input_tokens: 2_900_000,
    output_tokens: 120_000,
    reasoning_output_tokens: null,
    jobs: 23,
  };
  render(
    <UsageLimitsSection
      now={new Date(at)}
      onReset={vi.fn()}
      lastCall={null}
      tokens={{ today: usage, last_7_days: usage, this_month: usage }}
    />,
  );
  expect(screen.getByText("Today: 3.4 M input (2.9 M cached), 120 k output tokens in 23 jobs")).toBeVisible();
  expect(screen.getByText(/^Last 7 days:/)).toHaveAccessibleDescription(/Reported by the provider/);
});

it("rules are initially collapsed", async () => {
  render(<RepoSettingsCard settings={repoSettings} />);
  expect(screen.getByText("Show the 2 built-in rules").closest("details")).not.toHaveAttribute("open");
  await userEvent.click(screen.getByText(`Show the ${repoSettings.rules.length} built-in rules`));
  expect(screen.getByRole("table")).toBeVisible();
});
