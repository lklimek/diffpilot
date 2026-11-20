import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { describe, expect, it } from "vitest";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { expectTrustWording } from "../../__tests__/wording";
import { SETTINGS, setScenario } from "../../mocks/scenarios";

const server = startMockServer();
const region = (name: string | RegExp) => screen.findByRole("region", { name });

/** The signed-out Home: Settings shows its own sign-in card until the route change commits, so wait for it to go. */
async function expectSignedOutHome(router: ReturnType<typeof renderRoute>) {
  await waitFor(() => {
    expect(router.state.location.pathname).toBe("/");
    expect(screen.queryByRole("heading", { level: 1, name: "Settings" })).toBeNull();
  });
  expect(await screen.findByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
}

describe("Settings (brief §4.4)", () => {
  it("default: LLM working with the time of the last call, language, App, repository rules", async () => {
    renderRoute("/settings");
    expect(await screen.findByRole("heading", { level: 1, name: "Settings" })).toBeVisible();
    const llm = await region("LLM status");
    expect(within(llm).getByText("working")).toBeVisible();
    expect(llm).toHaveTextContent("last LLM call:");
    expect(screen.getByLabelText(/written in/)).toHaveValue("en");
    expect(await region("GitHub App")).toHaveTextContent("acme: selected repositories");
    const repo = await region(/acme\/node-client/);
    await userEvent.click(await within(repo).findByText(/Show the .* built-in rules/));
    expect(await within(repo).findByText("tests/**")).toBeVisible();
    expect(repo).toHaveTextContent("acme/node-sdk");
    expectTrustWording(document.body.textContent ?? "");
  });

  it("LLM not working: the plain message", async () => {
    setScenario("llm-error");
    renderRoute("/settings");
    const llm = await region("LLM status");
    expect(await within(llm).findByText("not working")).toBeVisible();
    expect(llm).toHaveTextContent("The LLM provider's usage limit was reached.");
  });

  it("installations not checked: said plainly, the rest still shown", async () => {
    setScenario("installations-not-checked");
    renderRoute("/settings");
    const app = await region("GitHub App");
    expect(app).toHaveTextContent("GitHub App installations: not checked. Signing in again may help.");
    expect(within(app).getByRole("link", { name: "Sign in with GitHub" })).toHaveAttribute(
      "href",
      "/auth/github/login",
    );
    expect(await region("LLM status")).toHaveTextContent("working");
  });

  it("repository settings not known yet: a neutral note, not an error", async () => {
    const notKnown =
      "Not known yet: the repository settings are read during an analysis and appear once one has run.";
    server.use(
      http.get("/api/v1/repos/:owner/:name/settings", () =>
        HttpResponse.json({ code: "analysis_not_finished", message: notKnown }, { status: 409 }),
      ),
    );
    renderRoute("/settings");
    const note = await screen.findByText(notKnown);
    expect(note).toHaveAttribute("role", "status");
    expect(screen.queryByRole("alert")).toBeNull();
    // The note sits in the repository's card, like the facts it stands in for.
    expect(within(await region(/^Repository acme\/node-client/)).getByText(notKnown)).toBe(note);
  });

  it("no LLM call yet, no profile, no related repositories: said, not hidden", async () => {
    setScenario("llm-unknown");
    renderRoute("/settings");
    expect(await region("LLM status")).toHaveTextContent("not known yet");
    expect(screen.getByLabelText(/written in/)).toHaveValue("pl");
    // The card shows "Loading…" first, then the facts.
    const repo = () => screen.getByRole("region", { name: /acme\/node-client/ });
    await waitFor(() => expect(repo()).toHaveTextContent("No project profile"));
    expect(repo()).toHaveTextContent("None detected");
  });

  it("changing the language saves it", async () => {
    renderRoute("/settings");
    await region("LLM status");
    await userEvent.selectOptions(screen.getByLabelText(/written in/), "pl");
    await waitFor(() => expect(screen.getByLabelText(/written in/)).toHaveValue("pl"));
    await waitFor(() => expect(screen.getByLabelText(/written in/)).toBeEnabled());
  });

  it("no repository analysed yet", async () => {
    server.use(http.get("/api/v1/settings", () => HttpResponse.json({ ...SETTINGS, repos: [] })));
    renderRoute("/settings");
    expect(
      await screen.findByText("No pull request has been analysed yet, so no repository settings exist."),
    ).toBeVisible();
  });

  it("an unknown ?repo= is said, with the picker; nothing else is shown in its place and the URL stays", async () => {
    const router = renderRoute("/settings?repo=acme/unknown");
    expect(await screen.findByText("Repository acme/unknown not found in this installation.")).toBeVisible();
    expect(screen.getByRole("combobox", { name: "Repository" })).toHaveValue("");
    expect(screen.queryByRole("region", { name: /^Repository acme\/node-client/ })).toBeNull();
    expect(router.state.location.search).toBe("?repo=acme/unknown");
    await userEvent.click(screen.getByRole("combobox", { name: "Repository" }));
    await userEvent.click(await screen.findByRole("option", { name: "acme/node-client" }));
    expect(await region(/^Repository acme\/node-client/)).toBeVisible();
    expect(router.state.location.search).toBe("?repo=acme%2Fnode-client");
  });

  it("settings cannot be read: plain error", async () => {
    server.use(
      http.get("/api/v1/settings", () =>
        HttpResponse.json(
          { code: "internal", message: "Something went wrong on the server." },
          { status: 500 },
        ),
      ),
    );
    renderRoute("/settings");
    expect(await screen.findByRole("alert")).toHaveTextContent("Something went wrong on the server.");
  });

  it("signed out: the sign-in card instead of settings", async () => {
    setScenario("signed-out");
    renderRoute("/settings");
    expect(await screen.findByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
    expect(screen.queryByRole("region", { name: "LLM status" })).toBeNull();
  });
});

it("Sign out ends the session, clears this login's drafts and shows the signed-out Home", async () => {
  window.sessionStorage.setItem("diffpilot.comment-drafts.alice.a1", JSON.stringify({ k: "mine" }));
  window.sessionStorage.setItem("diffpilot.comment-posts.alice.a1", JSON.stringify({}));
  window.sessionStorage.setItem("diffpilot.comment-drafts.bob.a1", JSON.stringify({ k: "bob's" }));
  let body: unknown;
  let csrf: string | null = null;
  server.events.on("request:start", async ({ request }) => {
    if (new URL(request.url).pathname === "/auth/logout") {
      csrf = request.headers.get("X-CSRF-Token");
      body = await request.clone().json();
    }
  });
  const router = renderRoute("/settings");
  await userEvent.click(within(await region("Account")).getByRole("button", { name: "Sign out" }));
  await expectSignedOutHome(router);
  expect(body).toEqual({ everywhere: false });
  expect(csrf).not.toBeNull();
  expect(window.sessionStorage.getItem("diffpilot.comment-drafts.alice.a1")).toBeNull();
  expect(window.sessionStorage.getItem("diffpilot.comment-posts.alice.a1")).toBeNull();
  expect(window.sessionStorage.getItem("diffpilot.comment-drafts.bob.a1")).toContain("bob's");
  window.sessionStorage.clear();
});

it("Sign out everywhere, once confirmed, ends every session and shows the signed-out Home", async () => {
  window.sessionStorage.setItem("diffpilot.comment-drafts.alice.a1", JSON.stringify({ k: "mine" }));
  const bodies: unknown[] = [];
  server.events.on("request:start", async ({ request }) => {
    if (new URL(request.url).pathname === "/auth/logout") bodies.push(await request.clone().json());
  });
  const router = renderRoute("/settings");
  const account = await region("Account");
  await userEvent.click(within(account).getByRole("button", { name: "Sign out everywhere" }));
  expect(bodies).toEqual([]);
  await userEvent.click(within(account).getByRole("button", { name: "Yes, sign out everywhere" }));
  await expectSignedOutHome(router);
  expect(bodies).toEqual([{ everywhere: true }]);
  expect(window.sessionStorage.getItem("diffpilot.comment-drafts.alice.a1")).toBeNull();
  window.sessionStorage.clear();
});

it("Sign out of a session that already ended (401) still clears the drafts and shows the signed-out Home", async () => {
  window.sessionStorage.setItem("diffpilot.comment-drafts.alice.a1", JSON.stringify({ k: "mine" }));
  server.use(
    http.post("/auth/logout", () => {
      setScenario("signed-out");
      return HttpResponse.json({ code: "unauthenticated", message: "Sign in again." }, { status: 401 });
    }),
  );
  const router = renderRoute("/settings");
  await userEvent.click(within(await region("Account")).getByRole("button", { name: "Sign out" }));
  await expectSignedOutHome(router);
  expect(screen.queryByText(/Not signed out/)).toBeNull();
  expect(window.sessionStorage.getItem("diffpilot.comment-drafts.alice.a1")).toBeNull();
  window.sessionStorage.clear();
});

it("Sign out everywhere on an ended session (401) does not pass for success: the tokens were not revoked", async () => {
  server.use(
    http.post("/auth/logout", () => {
      setScenario("signed-out");
      return HttpResponse.json({ code: "unauthenticated", message: "Sign in again." }, { status: 401 });
    }),
  );
  renderRoute("/settings");
  const account = await region("Account");
  await userEvent.click(within(account).getByRole("button", { name: "Sign out everywhere" }));
  await userEvent.click(within(account).getByRole("button", { name: "Yes, sign out everywhere" }));
  // Whatever screen follows, the user must learn that nothing was revoked and how to retry.
  expect(await screen.findByRole("alert")).toHaveTextContent(/sign in/i);
  expect(screen.getByRole("alert")).toHaveTextContent(
    "Not signed out: your session had already ended, so nothing was revoked. Other sessions and personal access tokens still work; sign in and choose Sign out everywhere again.",
  );
  window.sessionStorage.clear();
});

it("Sign out that fails otherwise keeps everything and says so", async () => {
  window.sessionStorage.setItem("diffpilot.comment-drafts.alice.a1", JSON.stringify({ k: "mine" }));
  server.use(
    http.post("/auth/logout", () =>
      HttpResponse.json({ code: "internal", message: "The server failed." }, { status: 500 }),
    ),
  );
  const router = renderRoute("/settings");
  await userEvent.click(within(await region("Account")).getByRole("button", { name: "Sign out" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Not signed out");
  expect(router.state.location.pathname).toBe("/settings");
  expect(window.sessionStorage.getItem("diffpilot.comment-drafts.alice.a1")).toContain("mine");
  window.sessionStorage.clear();
});
