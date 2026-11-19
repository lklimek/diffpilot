import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { delay, http, HttpResponse } from "msw";
import { describe, expect, it, vi } from "vitest";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { expectTrustWording } from "../../__tests__/wording";
import type { AnalysisSummary, ResolvePrResponse } from "../../api/generated/types";
import { seedFor, setFixtureState } from "../../mocks/fixtures";
import { MOVED_HEAD, setScenario } from "../../mocks/scenarios";
import { SETTLE_MS } from "../../components/pr-check/PrCheckCard";

const server = startMockServer();
const analysis = seedFor("default", "analysis") as AnalysisSummary;
const PR_URL = "https://github.com/acme/node-client/pull/482";
const PROGRESS = `/analyses/${analysis.id}/progress`;

/** Types `url` and runs the check (lookup only: nothing starts). */
async function analyze(url = PR_URL) {
  const field = await screen.findByLabelText("GitHub pull request URL");
  await userEvent.clear(field);
  await userEvent.type(field, url);
  await userEvent.click(screen.getByRole("button", { name: "Check" }));
}

const card = () => screen.findByRole("region", { name: "Pull request check" });

const pageText = () => document.body.textContent ?? "";

/** The banner whose bold title is `title`, checked to be announced with `role`. */
async function banner(title: string, role: "status" | "alert") {
  const found = (await screen.findByText(title)).closest<HTMLElement>("[role]");
  expect(found).toHaveAttribute("role", role);
  return found!;
}

describe("Home (brief §4.1)", () => {
  it("signed out: the sign-in card with a primary button, no URL form", async () => {
    setScenario("signed-out");
    renderRoute("/review");
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
    renderRoute(`/review?login_error=${encodeURIComponent(code)}`);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(title);
    expect(alert).not.toHaveTextContent("<script>");
    expect(await screen.findByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
  });

  it("signed in: URL form with an example and the 'What happens after Start analysis' aside", async () => {
    renderRoute("/review");
    expect(await screen.findByRole("heading", { level: 1, name: "Which pull request?" })).toBeVisible();
    expect(screen.getByLabelText("GitHub pull request URL")).toHaveAccessibleDescription(
      /Example: https:\/\/github\.com/,
    );
    expect(screen.getByRole("complementary", { name: "What happens after Start analysis" })).toBeVisible();
    expect(screen.getByRole("banner")).toHaveTextContent("alice");
    expectTrustWording(pageText());
  });

  it("Check shows the PR in a card; nothing starts until Start analysis, which has the focus", async () => {
    let created = 0;
    server.events.on("request:start", ({ request }) => {
      if (request.method === "POST" && new URL(request.url).pathname === "/api/v1/analyses") created += 1;
    });
    const router = renderRoute("/review");
    await analyze();
    const result = await card();
    expect(result).toHaveTextContent("#482 feat(rpc): retry with backoff on timeout");
    expect(result.closest("[aria-live]")).not.toBeNull();
    const start = within(result).getByRole("button", { name: "Start analysis" });
    await waitFor(() => expect(start).toHaveFocus());
    expect(created).toBe(0);
    expect(router.state.location.pathname).toBe("/review");
    // The card reads as a whole: its findings describe the primary button, in a live region that is a real box.
    expect(start).toHaveAccessibleDescription(/#482|Starts the analysis/);
    expect(start.getAttribute("aria-describedby")?.split(" ")).toHaveLength(2);
    expect(getComputedStyle(result.closest("[aria-live]")!).display).not.toBe("contents");
    expect(result.closest("[aria-live]")).not.toHaveClass("contents");
    // A second, deliberate Enter starts it (not one in the same moment the card appears).
    await new Promise((resolve) => setTimeout(resolve, SETTLE_MS + 50));
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(router.state.location.pathname).toBe(PROGRESS));
    expect(created).toBe(1);
  });

  it("shows the check running on the field", async () => {
    server.use(
      http.post("/api/v1/pr/resolve", async () => {
        await delay(200);
        return HttpResponse.json(seedFor("default", "pr-resolve") as ResolvePrResponse);
      }),
    );
    renderRoute("/review");
    await analyze();
    const field = screen.getByLabelText("GitHub pull request URL");
    expect(field).toHaveAttribute("aria-busy", "true");
    expect(screen.getByRole("button", { name: "Checking…" })).toHaveAttribute("aria-disabled", "true");
    await card();
    expect(field).not.toHaveAttribute("aria-busy");
  });

  it("editing the field after a check clears the card: Start analysis never starts another PR", async () => {
    renderRoute("/review");
    await analyze();
    await card();
    const field = screen.getByLabelText("GitHub pull request URL");
    await userEvent.type(field, "9");
    expect(screen.queryByRole("region", { name: "Pull request check" })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Start analysis/ })).toBeNull();
    // Back to the checked URL: still no card until Check.
    await userEvent.type(field, "{Backspace}");
    expect(screen.queryByRole("region", { name: "Pull request check" })).toBeNull();
  });

  it("an existing analysis in another language: tagged, Open first, Analyse again in English", async () => {
    setScenario("existing-other-language");
    let created = 0;
    server.events.on("request:start", ({ request }) => {
      if (request.method === "POST" && new URL(request.url).pathname === "/api/v1/analyses") created += 1;
    });
    const router = renderRoute("/review");
    await analyze();
    const result = await card();
    expect(within(result).getByText("PL")).toHaveAccessibleDescription(
      "LLM texts of this analysis are in Polish.",
    );
    const open = within(result).getByRole("link", { name: "Open" });
    await waitFor(() => expect(open).toHaveFocus());
    const again = within(result).getByRole("button", { name: "Analyse again in English" });
    expect(again).toHaveAccessibleDescription(/English/);
    expect(created).toBe(0);
    await userEvent.click(again);
    await waitFor(() => expect(router.state.location.pathname).toBe(PROGRESS));
    expect(created).toBe(1);
  });

  it("an existing analysis in the user's language offers no second analysis", async () => {
    setFixtureState("resume");
    renderRoute("/review");
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
    renderRoute("/review");
    await analyze();
    expect(await card()).toHaveTextContent(
      "412 changed lines in 9 files. Planning: about 5 min (rough estimate from changed lines in recent runs).",
    );
  });

  it("Cancel clears the card and returns to the field", async () => {
    renderRoute("/review");
    await analyze();
    await userEvent.click(within(await card()).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("region", { name: "Pull request check" })).toBeNull();
    expect(screen.getByLabelText("GitHub pull request URL")).toHaveFocus();
  });

  it("a large PR: the size warning above Start analysis anyway; nothing starts without it", async () => {
    setScenario("large-pr");
    const router = renderRoute("/review");
    await analyze();
    const result = await card();
    expect(within(result).getByRole("note")).toHaveTextContent(
      "Large PR: 27 081 changed lines in 371 files.",
    );
    const start = within(result).getByRole("button", { name: "Start analysis anyway" });
    expect(
      within(result).getByRole("note").compareDocumentPosition(start) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(router.state.location.pathname).toBe("/review");
    await userEvent.click(start);
    await waitFor(() => expect(router.state.location.pathname).toBe(PROGRESS));
  });

  it.each([
    ["https://github.com/acme/node-client/issues/5", "This is not a GitHub pull request URL."],
    ["https://github.com/acme/private-repo/pull/1", "This repository is private."],
  ])("error for %s", async (url, message) => {
    const router = renderRoute("/review");
    await analyze(url);
    const alert = within(await card()).getByRole("alert");
    expect(alert).toHaveTextContent("Cannot open this pull request.");
    expect(alert).toHaveTextContent(message);
    expect(router.state.location.pathname).toBe("/review");
  });

  it("does not promise personal progress for an existing shared analysis", async () => {
    setFixtureState("resume");
    renderRoute("/review");
    await analyze();
    const message = await banner("Analysis found.", "status");
    expect(message).toHaveTextContent("This PR head is already analysed.");
    expect(message).not.toHaveTextContent("Your ticks, answers");
  });

  it("an existing finished analysis is resumed, nothing is analysed again", async () => {
    setFixtureState("resume");
    const router = renderRoute("/review");
    await analyze();
    const found = await banner("Analysis found.", "status");
    expect(found).toHaveTextContent("This PR head is already analysed.");
    expect(within(found).queryByRole("link", { name: /resume/i })).toBeNull();
    expect(within(found).getByRole("link", { name: "Open" })).toHaveAttribute(
      "href",
      `/analyses/${analysis.id}`,
    );
    expect(router.state.location.pathname).toBe("/review");
  });

  it("head moved: banner with both heads, Start analysis starts a fresh analysis, the old one stays reachable", async () => {
    setScenario("head-moved");
    const router = renderRoute("/review");
    await analyze();
    const moved = await banner("The PR has moved.", "alert");
    expect(moved).toHaveTextContent(
      `This analysis is for ${analysis.pr.head_sha.slice(0, 7)}; the PR is now at ${MOVED_HEAD.slice(0, 7)}.`,
    );
    expect(within(moved).getByRole("link", { name: "Open the old analysis" })).toHaveAccessibleDescription(
      "Opens the analysis of the older head. Starts nothing: no analysis, no LLM use.",
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
    await userEvent.click(within(await card()).getByRole("button", { name: "Start analysis" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(PROGRESS));
    expect(body).toMatchObject({ head_sha: MOVED_HEAD, pr: { number: 482 } });
  });

  it("an analysis in progress is shown, not started again", async () => {
    setScenario("existing-running");
    renderRoute("/review");
    await analyze();
    const found = await banner("An analysis of this PR is in progress.", "status");
    expect(within(found).getByRole("link", { name: "Show progress" })).toHaveAttribute("href", PROGRESS);
  });

  it("a paused analysis says until when and how far it got", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date("2026-11-17T14:20:00Z"));
    try {
      setScenario("existing-paused");
      renderRoute("/review");
      await analyze();
      const found = await banner("An analysis of this PR is in progress.", "status");
      expect(found).toHaveTextContent(/Paused until \d\d:\d\d — 2 of 5\./);
    } finally {
      vi.useRealTimers();
    }
  });

  it("a failed analysis shows its plain description and leads to Retry", async () => {
    setScenario("existing-failed");
    renderRoute("/review");
    await analyze();
    const failed = await banner("The last analysis of this PR failed.", "alert");
    expect(failed).toHaveTextContent("The analysis stopped because of a server error. Retry the analysis.");
    expect(within(failed).getByRole("link", { name: "Show details and retry" })).toHaveAttribute(
      "href",
      PROGRESS,
    );
  });

  it("a cancelled analysis is analysed again", async () => {
    setScenario("existing-cancelled");
    const router = renderRoute("/review");
    await analyze();
    await userEvent.click(within(await card()).getByRole("button", { name: "Start analysis" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(PROGRESS));
  });

  it("without the GitHub App: says what does not work and analyses on request", async () => {
    setScenario("no-app");
    const router = renderRoute("/review");
    await analyze();
    const noApp = await banner("Commenting is not available for acme/node-client right now.", "alert");
    expect(router.state.location.pathname).toBe("/review");
    expect(noApp).not.toHaveTextContent(/App is not installed/);
    // Neither the cause nor what else is affected is known when the check failed.
    expect(noApp).not.toHaveTextContent(/works without the App|related work/i);
    expect(noApp).toHaveTextContent("The analysis still runs");
    expect(router.state.location.pathname).toBe("/review");
    await userEvent.click(within(await card()).getByRole("button", { name: "Start analysis anyway" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(PROGRESS));
  });

  it("a PR given in the address (a dashboard row, a refused comment) is checked on arrival, never started", async () => {
    const router = renderRoute(`/review?pr=${encodeURIComponent(PR_URL)}`);
    expect(await screen.findByLabelText("GitHub pull request URL")).toHaveValue(PR_URL);
    expect(within(await card()).getByRole("button", { name: "Start analysis" })).toBeVisible();
    expect(screen.queryByText("Analysis cancelled. Check the PR to start again.")).toBeNull();
    expect(router.state.location.pathname).toBe("/review");
  });

  it("after a cancelled analysis: 'Analysis cancelled. Check the PR to start again.' with the URL filled in", async () => {
    renderRoute({ pathname: "/review", state: { cancelledPrUrl: PR_URL } });
    await banner("Analysis cancelled. Check the PR to start again.", "status");
    expect(screen.getByLabelText("GitHub pull request URL")).toHaveValue(PR_URL);
  });

  it("server down: plain error", async () => {
    server.use(http.get("/api/v1/me", () => HttpResponse.error()));
    renderRoute("/review");
    expect(await screen.findByRole("alert")).toHaveTextContent("Cannot reach the diffpilot server.");
  });

  it("an edit of the field while the start is on its way does not lose the started analysis", async () => {
    server.use(
      http.post("/api/v1/analyses", async () => {
        await delay(300);
        return HttpResponse.json(analysis, { status: 201 });
      }),
    );
    const router = renderRoute("/review");
    await analyze();
    await userEvent.click(within(await card()).getByRole("button", { name: "Start analysis" }));
    await userEvent.type(screen.getByLabelText("GitHub pull request URL"), "9");
    await waitFor(() => expect(router.state.location.pathname).toBe(PROGRESS));
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
    const router = renderRoute("/review");
    await analyze();
    await userEvent.click(within(await card()).getByRole("button", { name: "Start analysis" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Cannot start the analysis.");
    expect(alert).toHaveTextContent("An analysis of this head is already running.");
    expect(router.state.location.pathname).toBe("/review");
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
  renderRoute("/review");
  await analyze();
  const found = await banner("Analysis found.", "status");
  expect(found).toHaveTextContent("Your ticks, answers and last viewed block are kept");
  expect(within(found).getByRole("link", { name: "Open" })).toHaveAttribute(
    "href",
    `/analyses/${analysis.id}`,
  );
});

it.each([
  [true, true],
  [false, false],
])(
  "a review-ready analysis in progress (started: %s): Open, tooltip keeps ticks only if started",
  async (started, kept) => {
    server.use(
      http.post("/api/v1/pr/resolve", () =>
        HttpResponse.json({
          pr: analysis.pr,
          commenting_available: true,
          head_moved: false,
          analysis: {
            ...analysis,
            state: { state: "running", done: 2, total: 5 },
            review_ready: true,
            has_my_state: started,
          },
        }),
      ),
    );
    renderRoute("/review");
    await analyze();
    const found = await banner("An analysis of this PR is in progress.", "status");
    const open = within(found).getByRole("link", { name: "Open" });
    expect(open).toHaveAttribute("href", `/analyses/${analysis.id}`);
    expect(open).toHaveAccessibleDescription(/Starts nothing: no analysis, no LLM use\.$/);
    const description = open.getAttribute("aria-describedby") ?? "";
    expect(document.getElementById(description)?.textContent?.includes("ticks and answers are kept")).toBe(
      kept,
    );
    expect(document.getElementById(description)?.textContent).not.toMatch(/continues/);
  },
);

describe("Recent reviews", () => {
  it("lists the user's own analyses under the form; a row opens its analysis", async () => {
    const router = renderRoute("/review");
    const section = await screen.findByRole("region", { name: "Recent reviews" });
    const items = await within(section).findAllByRole("listitem");
    expect(items).toHaveLength(3);
    const [one, two, three] = items as [HTMLElement, HTMLElement, HTMLElement];
    expect(one).toHaveTextContent(`acme/node-client #482`);
    expect(one).toHaveTextContent(analysis.pr.title);
    expect(two).toHaveTextContent("acme/wallet-ui #96");
    expect(two).toHaveTextContent("Analysing flows");
    expect(three).toHaveTextContent("Analysis failed");
    // The form comes first in reading order.
    const field = screen.getByLabelText("GitHub pull request URL");
    expect(field.compareDocumentPosition(section) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const link = within(one).getByRole("link");
    await userEvent.click(link);
    await waitFor(() =>
      expect(router.state.location.pathname).toMatch(new RegExp(`^/analyses/${analysis.id}`)),
    );
    expectTrustWording(section.textContent ?? "");
  });

  it("empty state when the user has started none", async () => {
    setScenario("no-reviews");
    renderRoute("/review");
    const section = await screen.findByRole("region", { name: "Recent reviews" });
    expect(
      await within(section).findByText(/No reviews yet\. Paste a PR URL above to start\./),
    ).toBeVisible();
  });

  it("an error loading the list is shown in the section; the form still works", async () => {
    server.use(
      http.get("/api/v1/analyses", () =>
        HttpResponse.json({ code: "unavailable", message: "The server is busy." }, { status: 500 }),
      ),
    );
    renderRoute("/review");
    const section = await screen.findByRole("region", { name: "Recent reviews" });
    expect(await within(section).findByRole("alert")).toHaveTextContent(
      "Recent reviews could not be loaded. The server is busy.",
    );
    expect(screen.getByLabelText("GitHub pull request URL")).toBeEnabled();
  });

  it("is not shown signed out", async () => {
    setScenario("signed-out");
    renderRoute("/review");
    await screen.findByRole("link", { name: "Sign in with GitHub" });
    expect(screen.queryByRole("region", { name: "Recent reviews" })).toBeNull();
  });
});
