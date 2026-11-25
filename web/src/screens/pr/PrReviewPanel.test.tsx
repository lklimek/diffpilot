import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { delay, http, HttpResponse } from "msw";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { expectTrustWording } from "../../__tests__/wording";
import type { AnalysisSummary, ResolvePrResponse } from "../../api/generated/types";
import { seedFor, setFixtureState } from "../../mocks/fixtures";
import { MOVED_HEAD, setScenario } from "../../mocks/scenarios";

const server = startMockServer();

beforeEach(() => {
  FakeEventSource.reset();
  vi.stubGlobal("EventSource", FakeEventSource);
});
afterEach(() => vi.unstubAllGlobals());
const analysis = seedFor("default", "analysis") as AnalysisSummary;
const PR_URL = "https://github.com/acme/node-client/pull/482";
const PR = "/pr/acme/node-client/482";

/** Types `url` and opens the PR (lookup only: nothing starts). */
async function analyze(url = PR_URL) {
  const field = await screen.findByLabelText("GitHub pull request URL");
  await userEvent.clear(field);
  await userEvent.type(field, url);
  await userEvent.click(screen.getByRole("button", { name: "Open" }));
}

const card = async () => {
  const panel = await screen.findByRole("region", { name: "Review the code" });
  await waitFor(() => expect(within(panel).queryByText("Checking the pull request…")).toBeNull());
  return panel;
};

const pageText = () => document.body.textContent ?? "";

/** The banner whose bold title is `title`, checked to be announced with `role`. */
async function banner(title: string, role: "status" | "alert") {
  const found = (await screen.findByText(title)).closest<HTMLElement>("[role]");
  expect(found).toHaveAttribute("role", role);
  return found!;
}

describe("Pull requests and Review the code (U59–U61)", () => {
  it("signed out: the sign-in card with a primary button, no URL form", async () => {
    setScenario("signed-out");
    renderRoute("/prs");
    expect(await screen.findByRole("link", { name: "Sign in with GitHub" })).toHaveAttribute(
      "href",
      "/auth/github/login",
    );
    expect(screen.queryByLabelText("GitHub pull request URL")).toBeNull();
    expectTrustWording(pageText());
  });

  it.each([
    ["login_not_allowed", "This GitHub login is not allowed."],
    ["oauth_failed", "Sign-in with GitHub did not complete."],
    ["state_mismatch", "The sign-in expired."],
    ["<script>", "Sign-in failed."],
  ])("login error %s is explained in plain words", async (code, title) => {
    setScenario("signed-out");
    renderRoute(`/prs?login_error=${encodeURIComponent(code)}`);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(title);
    expect(alert).not.toHaveTextContent("<script>");
    expect(await screen.findByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
  });

  it("signed in: shows the opener and the example", async () => {
    renderRoute("/prs");
    expect(await screen.findByLabelText("GitHub pull request URL")).toHaveAttribute(
      "placeholder",
      "https://github.com/owner/repo/pull/123 or owner/repo#123",
    );
    expect(screen.getByRole("banner")).toHaveTextContent("alice");
    expectTrustWording(pageText());
  });

  it("announces the pending check on the PR page", async () => {
    server.use(
      http.post("/api/v1/pr/resolve", async () => {
        await delay(400);
        return HttpResponse.json(seedFor("default", "pr-resolve") as ResolvePrResponse);
      }),
    );
    renderRoute("/prs");
    await analyze();
    expect(await screen.findByText("Checking the pull request…")).toBeVisible();
    expect(screen.queryByRole("button", { name: "Start analysis" })).toBeNull();
    expect(await screen.findByRole("button", { name: "Start analysis" })).toBeVisible();
  });

  it("an existing analysis in another language: tagged, Open first, Analyse again in English", async () => {
    setScenario("existing-other-language");
    let created = 0;
    server.events.on("request:start", ({ request }) => {
      if (request.method === "POST" && new URL(request.url).pathname === "/api/v1/analyses") created += 1;
    });
    const router = renderRoute("/prs");
    await analyze();
    const result = await card();
    expect(within(result).getByText("PL")).toHaveAccessibleDescription(
      "LLM texts of this analysis are in Polish.",
    );
    const open = within(result).getByRole("link", { name: "Open review" });
    expect(open).toBeVisible();
    const again = within(result).getByRole("button", { name: "Analyse again in English" });
    expect(again).toHaveAccessibleDescription(/English/);
    expect(created).toBe(0);
    await userEvent.click(again);
    await within(await card()).findByText("Connecting…");
    expect(router.state.location.pathname).toBe(PR);
    expect(created).toBe(1);
  });

  it("an existing analysis in the user's language offers no second analysis", async () => {
    setFixtureState("resume");
    renderRoute("/prs");
    await analyze();
    const result = await card();
    expect(within(result).getByText("EN")).toBeVisible();
    expect(within(result).queryByRole("button", { name: /Analyse again/ })).toBeNull();
  });

  it("the card shows the PR's size and the planning estimate when the lookup has them", async () => {
    server.use(
      http.post("/api/v1/pr/resolve", () =>
        HttpResponse.json({
          ...(seedFor("default", "pr-resolve") as ResolvePrResponse),
          size: { changed_lines: 412, changed_files: 9 },
          planning_estimate: { remaining_seconds: 300, basis: "lines_only", rough: true },
        }),
      ),
    );
    renderRoute("/prs");
    await analyze();
    expect(await card()).toHaveTextContent(
      "412 changed lines in 9 files. Planning: about 5 min (rough estimate from changed lines in recent runs).",
    );
  });

  it("a large PR: the size warning above Start analysis anyway; nothing starts without it", async () => {
    setScenario("large-pr");
    const router = renderRoute("/prs");
    await analyze();
    const result = await card();
    expect(within(result).getByRole("note")).toHaveTextContent(
      "Large PR: 27 081 changed lines in 371 files.",
    );
    const start = within(result).getByRole("button", { name: "Start analysis anyway" });
    expect(
      within(result).getByRole("note").compareDocumentPosition(start) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(router.state.location.pathname).toBe(PR);
    await userEvent.click(start);
    await within(await card()).findByText("Connecting…");
    expect(router.state.location.pathname).toBe(PR);
  });

  it.each([
    ["https://github.com/acme/node-client/issues/5", "Enter a GitHub pull request link", "/prs"],
    ["https://github.com/acme/private-repo/pull/1", "This repository is private.", "/pr/acme/private-repo/1"],
  ])("error for %s", async (url, message, pathname) => {
    const router = renderRoute("/prs");
    await analyze(url);
    expect(await screen.findByRole("alert")).toHaveTextContent(message);
    expect(router.state.location.pathname).toBe(pathname);
  });

  it("does not promise personal progress for an existing shared analysis", async () => {
    setFixtureState("resume");
    renderRoute("/prs");
    await analyze();
    const message = await banner("Analysis found.", "status");
    expect(message).toHaveTextContent("This PR head is already analysed.");
    expect(message).not.toHaveTextContent("Your ticks, answers");
  });

  it("an existing finished analysis is resumed, nothing is analysed again", async () => {
    setFixtureState("resume");
    const router = renderRoute("/prs");
    await analyze();
    const found = await banner("Analysis found.", "status");
    expect(found).toHaveTextContent("This PR head is already analysed.");
    expect(within(found).queryByRole("link", { name: /resume/i })).toBeNull();
    expect(within(found).getByRole("link", { name: "Open review" })).toHaveAttribute(
      "href",
      `/analyses/${analysis.id}`,
    );
    expect(router.state.location.pathname).toBe(PR);
  });

  it("head moved: banner with both heads, Start analysis starts a fresh analysis, the old one stays reachable", async () => {
    setScenario("head-moved");
    const router = renderRoute("/prs");
    await analyze();
    const moved = await banner("The PR has moved.", "alert");
    expect(moved).toHaveTextContent(
      `Analysis of ${analysis.pr.head_sha.slice(0, 7)}; the PR is now at ${MOVED_HEAD.slice(0, 7)}.`,
    );
    expect(within(moved).getByRole("link", { name: "Open the old analysis" })).toHaveAccessibleDescription(
      "Opens the analysis of the older head.",
    );
    expect(within(moved).getByRole("link", { name: "Open the old analysis" })).toHaveAttribute(
      "href",
      `/analyses/${analysis.id}`,
    );
    let body: unknown;
    server.events.on("request:start", async ({ request }) => {
      if (request.method === "POST" && new URL(request.url).pathname === "/api/v1/analyses") {
        body = await request.clone().json();
      }
    });
    await userEvent.click(
      within(await card()).getByRole("button", { name: "Start analysis of the new head" }),
    );
    await within(await card()).findByText("Connecting…");
    expect(router.state.location.pathname).toBe(PR);
    expect(body).toMatchObject({ head_sha: MOVED_HEAD, pr: { number: 482 } });
  });

  it("a cancelled analysis is analysed again", async () => {
    setScenario("existing-cancelled");
    const router = renderRoute("/prs");
    await analyze();
    await userEvent.click(within(await card()).getByRole("button", { name: "Start analysis" }));
    await within(await card()).findByText("Connecting…");
    expect(router.state.location.pathname).toBe(PR);
  });

  it("without the GitHub App: says what does not work and analyses on request", async () => {
    setScenario("no-app");
    const router = renderRoute("/prs");
    await analyze();
    const noApp = await banner("Commenting is not available for acme/node-client right now.", "alert");
    expect(router.state.location.pathname).toBe(PR);
    expect(noApp).not.toHaveTextContent(/App is not installed/);
    // Neither the cause nor what else is affected is known when the check failed.
    expect(noApp).not.toHaveTextContent(/works without the App|related work/i);
    expect(noApp).toHaveTextContent("The analysis still runs");
    expect(within(noApp).getByRole("link", { name: "GitHub sync" })).toHaveAttribute(
      "href",
      "/settings/github-sync",
    );
    expect(router.state.location.pathname).toBe(PR);
    await userEvent.click(within(await card()).getByRole("button", { name: "Start analysis anyway" }));
    await within(await card()).findByText("Connecting…");
    expect(router.state.location.pathname).toBe(PR);
  });

  it("server down: plain error", async () => {
    server.use(http.get("/api/v1/me", () => HttpResponse.error()));
    renderRoute("/prs");
    expect(await screen.findByRole("alert")).toHaveTextContent("Cannot reach the diffpilot server.");
  });

  it("the analysis cannot be started: plain error, stays on Home", async () => {
    server.use(
      http.post("/api/v1/analyses", () =>
        HttpResponse.json(
          { code: "conflict", message: "An analysis of this head is already running." },
          { status: 409 },
        ),
      ),
    );
    const router = renderRoute("/prs");
    await analyze();
    await userEvent.click(within(await card()).getByRole("button", { name: "Start analysis" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Cannot start the analysis.");
    expect(alert).toHaveTextContent("An analysis of this head is already running.");
    expect(router.state.location.pathname).toBe(PR);
  });
});

it("only promises personal state when the server confirms it exists", async () => {
  server.use(
    http.post("/api/v1/pr/resolve", () =>
      HttpResponse.json({
        pr: analysis.pr,
        commenting_available: true,
        head_moved: false,
        analysis: { ...analysis, state: { state: "finished", total: 5 }, has_my_state: true },
      }),
    ),
  );
  renderRoute("/prs");
  await analyze();
  const found = await banner("Analysis found.", "status");
  expect(found).toHaveTextContent("Your ticks, answers and last viewed block are kept");
  expect(within(found).getByRole("link", { name: "Open review" })).toHaveAttribute(
    "href",
    `/analyses/${analysis.id}`,
  );
});
