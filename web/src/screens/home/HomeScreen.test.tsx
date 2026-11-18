import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { describe, expect, it } from "vitest";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { expectTrustWording } from "../../__tests__/wording";
import type { AnalysisSummary } from "../../api/generated/types";
import { seedFor, setFixtureState } from "../../mocks/fixtures";
import { MOVED_HEAD, setScenario } from "../../mocks/scenarios";

const server = startMockServer();
const analysis = seedFor("default", "analysis") as AnalysisSummary;
const PR_URL = "https://github.com/acme/node-client/pull/482";
const PROGRESS = `/analyses/${analysis.id}/progress`;

async function analyze(url = PR_URL) {
  const field = await screen.findByLabelText("GitHub pull request URL");
  await userEvent.clear(field);
  await userEvent.type(field, url);
  await userEvent.click(screen.getByRole("button", { name: "Analyze" }));
}

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
    renderRoute("/");
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
    renderRoute(`/?login_error=${encodeURIComponent(code)}`);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(title);
    expect(alert).not.toHaveTextContent("<script>");
    expect(await screen.findByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
  });

  it("signed in: URL form with an example and the 'What happens after Analyze' aside", async () => {
    renderRoute("/");
    expect(await screen.findByRole("heading", { level: 1, name: "Which pull request?" })).toBeVisible();
    expect(screen.getByLabelText("GitHub pull request URL")).toHaveAccessibleDescription(
      /Example: https:\/\/github\.com/,
    );
    expect(screen.getByRole("complementary", { name: "What happens after Analyze" })).toBeVisible();
    expect(screen.getByRole("banner")).toHaveTextContent("alice");
    expectTrustWording(pageText());
  });

  it("a PR without an analysis starts one and opens the progress screen", async () => {
    const router = renderRoute("/");
    await analyze();
    await waitFor(() => expect(router.state.location.pathname).toBe(PROGRESS));
  });

  it.each([
    ["https://github.com/acme/node-client/issues/5", "This is not a GitHub pull request URL."],
    ["https://github.com/acme/private-repo/pull/1", "This repository is private."],
  ])("error for %s", async (url, message) => {
    const router = renderRoute("/");
    await analyze(url);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Cannot open this pull request.");
    expect(alert).toHaveTextContent(message);
    expect(router.state.location.pathname).toBe("/");
  });

  it("does not promise personal progress for an existing shared analysis", async () => {
    setFixtureState("resume");
    renderRoute("/");
    await analyze();
    const message = await banner("Analysis found.", "status");
    expect(message).toHaveTextContent("This PR head is already analysed. Resume review.");
    expect(message).not.toHaveTextContent("Your ticks, answers");
  });

  it("an existing finished analysis is resumed, nothing is analysed again", async () => {
    setFixtureState("resume");
    const router = renderRoute("/");
    await analyze();
    const found = await banner("Analysis found.", "status");
    expect(found).toHaveTextContent("This PR head is already analysed. Resume review.");
    expect(within(found).getByRole("link", { name: "Resume review" })).toHaveAttribute(
      "href",
      `/analyses/${analysis.id}`,
    );
    expect(router.state.location.pathname).toBe("/");
  });

  it("head moved: banner with both heads, Re-analyze starts a fresh analysis, the old one stays reachable", async () => {
    setScenario("head-moved");
    const router = renderRoute("/");
    await analyze();
    const moved = await banner("The PR has moved.", "alert");
    expect(moved).toHaveTextContent(
      `This analysis is for ${analysis.pr.head_sha.slice(0, 7)}; the PR is now at ${MOVED_HEAD.slice(0, 7)}.`,
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
    await userEvent.click(within(moved).getByRole("button", { name: "Re-analyze" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(PROGRESS));
    expect(body).toMatchObject({ head_sha: MOVED_HEAD, pr: { number: 482 } });
  });

  it("an analysis in progress is shown, not started again", async () => {
    setScenario("existing-running");
    renderRoute("/");
    await analyze();
    const found = await banner("An analysis of this PR is in progress.", "status");
    expect(within(found).getByRole("link", { name: "Show progress" })).toHaveAttribute("href", PROGRESS);
  });

  it("a paused analysis says until when and how far it got", async () => {
    setScenario("existing-paused");
    renderRoute("/");
    await analyze();
    const found = await banner("An analysis of this PR is in progress.", "status");
    expect(found).toHaveTextContent(/Paused until \d\d:\d\d — 2 of 5\./);
  });

  it("a failed analysis shows its plain description and leads to Retry", async () => {
    setScenario("existing-failed");
    renderRoute("/");
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
    const router = renderRoute("/");
    await analyze();
    await waitFor(() => expect(router.state.location.pathname).toBe(PROGRESS));
  });

  it("without the GitHub App: says what does not work and analyses on request", async () => {
    setScenario("no-app");
    const router = renderRoute("/");
    await analyze();
    const noApp = await banner("Commenting is not available for acme/node-client right now.", "alert");
    expect(noApp).not.toHaveTextContent(/App is not installed/);
    expect(router.state.location.pathname).toBe("/");
    await userEvent.click(within(noApp).getByRole("button", { name: "Analyze anyway" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(PROGRESS));
  });

  it("after a cancelled analysis: 'Analysis cancelled — analyze again' with the URL filled in", async () => {
    renderRoute({ pathname: "/", state: { cancelledPrUrl: PR_URL } });
    await banner("Analysis cancelled — analyze again.", "status");
    expect(screen.getByLabelText("GitHub pull request URL")).toHaveValue(PR_URL);
  });

  it("server down: plain error", async () => {
    server.use(http.get("/api/v1/me", () => HttpResponse.error()));
    renderRoute("/");
    expect(await screen.findByRole("alert")).toHaveTextContent("Cannot reach the smart-review server.");
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
    const router = renderRoute("/");
    await analyze();
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Cannot start the analysis.");
    expect(alert).toHaveTextContent("An analysis of this head is already running.");
    expect(router.state.location.pathname).toBe("/");
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
  renderRoute("/");
  await analyze();
  expect(await banner("Analysis found.", "status")).toHaveTextContent(
    "Your ticks, answers and last viewed block are kept",
  );
});
