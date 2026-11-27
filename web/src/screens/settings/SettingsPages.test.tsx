import { QueryClient } from "@tanstack/react-query";
import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { expect, it } from "vitest";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { SETTINGS } from "../../mocks/scenarios";

const server = startMockServer();

it("redirects old repository links and opens the read-only rules in the requested table row", async () => {
  const router = renderRoute("/settings?repo=acme%2Fnode-client");
  const table = await screen.findByRole("table", { name: "Repositories" });
  expect(router.state.location.pathname).toBe("/settings/repositories");
  expect(
    await within(table).findByRole("button", { name: "Hide analysis rules for acme/node-client" }),
  ).toHaveAttribute("aria-expanded", "true");
  expect(await within(table).findByText("Project profile source")).toBeVisible();
  expect(within(table).getAllByText("App not installed")[0]).toBeVisible();
  const name = await within(table).findByLabelText("Short name of acme/live-only");
  expect(name).toHaveValue("");
  expect(name).toHaveAttribute("placeholder", "live-only");
});

it("keeps the settings links under the parent and marks just the current page when collapsed", async () => {
  renderRoute("/settings/memcan");
  const nav = within(await screen.findByRole("navigation", { name: "Main" }));
  await userEvent.click(nav.getByRole("button", { name: "Collapse navigation" }));
  expect(nav.getByRole("link", { name: "Settings" })).not.toHaveAttribute("aria-current");
  expect(nav.getByRole("link", { name: "MemCan" })).toHaveAttribute("aria-current", "page");
  expect(
    nav
      .getAllByRole("link")
      .slice(-6)
      .map((link) => link.getAttribute("aria-label")),
  ).toEqual(["Account", "LLM & usage", "GitHub sync", "Repositories", "Review loop", "MemCan"]);
  expect(await screen.findByRole("region", { name: "MemCan status" })).toHaveTextContent("project profiles");
});

it("requires confirmation before resetting only today's ranking count", async () => {
  renderRoute("/settings/llm");
  const ranking = within(await screen.findByRole("region", { name: "Ranking today" }));
  expect(await ranking.findByText("37 of 120 items sent for judgement")).toBeVisible();
  await userEvent.click(ranking.getByRole("button", { name: "Reset my limits" }));
  expect(ranking.getByText("37 of 120 items sent for judgement")).toBeVisible();
  await userEvent.click(ranking.getByRole("button", { name: "Reset today's count" }));
  expect(await ranking.findByText("0 of 120 items sent for judgement")).toBeVisible();
  expect(ranking.getByText("Up to 48 per refresh")).toBeVisible();
});

it("holds a model or effort change until Save and guards switching settings pages", async () => {
  const router = renderRoute("/settings/llm");
  const model = within(await screen.findByRole("region", { name: "Model and effort" }));
  await userEvent.selectOptions(model.getByLabelText("Ask effort"), "low");
  await userEvent.click(screen.getByRole("link", { name: "Repositories" }));
  const prompt = await screen.findByRole("alertdialog", { name: "Unsaved settings" });
  expect(router.state.location.pathname).toBe("/settings/llm");
  await userEvent.click(within(prompt).getByRole("button", { name: "Stay" }));
  await userEvent.click(model.getByRole("button", { name: "Save" }));
  expect(await model.findByText("Saved")).toBeVisible();
  await userEvent.click(screen.getByRole("link", { name: "Repositories" }));
  await waitFor(() => expect(router.state.location.pathname).toBe("/settings/repositories"));
});

it("saves three concurrent LLM job slots without changing the model", async () => {
  const writes: unknown[] = [];
  server.events.on("request:start", async ({ request }) => {
    if (request.method === "PUT" && new URL(request.url).pathname === "/api/v1/settings/llm/job-slots")
      writes.push(await request.clone().json());
  });
  renderRoute("/settings/llm");
  const model = within(await screen.findByRole("region", { name: "Model and effort" }));
  const slots = model.getByRole("spinbutton", { name: "Concurrent LLM jobs" });
  expect(slots).toHaveValue(1);
  await userEvent.clear(slots);
  await userEvent.type(slots, "0");
  const save = model.getByRole("button", { name: "Save" });
  expect(save).toHaveAttribute("aria-disabled", "true");
  expect(slots).toHaveAccessibleDescription("Enter a whole number from 1 to 65,535.");
  await userEvent.click(save);
  expect(writes).toEqual([]);
  await userEvent.clear(slots);
  await userEvent.type(slots, "3");
  expect(writes).toEqual([]);
  await userEvent.click(model.getByRole("button", { name: "Save" }));
  expect(await model.findByText("Saved")).toBeVisible();
  expect(slots).toHaveValue(3);
  expect(model.getByLabelText("Model")).toHaveValue("gpt-6.1-sol");
  expect(writes).toEqual([{ slots: 3 }]);
});

it.each(["model", "language"] as const)(
  "drops a stale %s edit and its settings cache when the browser changes accounts",
  async (field) => {
    const original = { login: "alice", csrf: "alice-session", model: "gpt-6.1-sol", language: "en" };
    const colleague = { login: "bob", csrf: "bob-session", model: "gpt-6-sol", language: "ru" };
    let signedIn = original;
    const reads: (string | null)[] = [];
    const writes: (string | null)[] = [];
    const changedAccounts = () =>
      HttpResponse.json({ code: "csrf_rejected", message: "Reload the page." }, { status: 403 });
    server.use(
      http.get("/api/v1/me", () =>
        HttpResponse.json({
          user: { login: signedIn.login, name: null, avatar_url: null, language: signedIn.language },
          csrf_token: signedIn.csrf,
        }),
      ),
      http.get("/api/v1/settings", ({ request }) => {
        const token = request.headers.get("X-CSRF-Token");
        reads.push(token);
        if (token !== null && token !== signedIn.csrf) return changedAccounts();
        return HttpResponse.json({
          ...SETTINGS,
          language: signedIn.language,
          llm_selection: {
            ...SETTINGS.llm_selection!,
            current: { model: signedIn.model, reasoning_effort: "high" },
          },
        });
      }),
      http.put(`/api/v1/settings/${field === "model" ? "llm" : "language"}`, async ({ request }) => {
        const token = request.headers.get("X-CSRF-Token");
        writes.push(token);
        if (token !== signedIn.csrf) return changedAccounts();
        if (field === "language") {
          signedIn.language = ((await request.json()) as { language: string }).language;
          return new HttpResponse(null, { status: 204 });
        }
        return HttpResponse.json({ ...SETTINGS.llm_selection!, current: await request.json() });
      }),
    );
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    renderRoute("/settings/llm", queryClient);
    const card = within(await screen.findByRole("region", { name: "Model and effort" }));
    expect(card.getByLabelText("Model")).toHaveValue(original.model);
    if (field === "model") {
      await userEvent.selectOptions(card.getByLabelText("Model"), "gpt-6-astra");
      signedIn = colleague;
      await userEvent.click(card.getByRole("button", { name: "Save" }));
    } else {
      const language = screen.getByLabelText("Explanations, questions and assessments are written in");
      expect(language).toHaveValue(original.language);
      signedIn = colleague;
      await userEvent.selectOptions(language, "pl");
    }

    await waitFor(() => expect(screen.getByLabelText("Model")).toHaveValue(colleague.model));
    expect(writes).toEqual([original.csrf]);
    expect(reads).toEqual(expect.arrayContaining([original.csrf, colleague.csrf]));
    expect(reads).not.toContain(null);
    const entries = queryClient.getQueriesData<typeof SETTINGS>({ queryKey: ["settings", "session"] });
    expect(entries.map(([, data]) => data?.llm_selection?.current.model)).toEqual([colleague.model]);
    const cacheKeys = JSON.stringify(entries.map(([key]) => key));
    expect(cacheKeys).not.toContain(original.csrf);
    expect(cacheKeys).not.toContain(colleague.csrf);
    expect(colleague.language).toBe("ru");
    if (field === "language") {
      const readCount = reads.length;
      const language = screen.getByLabelText("Explanations, questions and assessments are written in");
      expect(language).toHaveValue("ru");
      await userEvent.selectOptions(language, "pl");
      await waitFor(() => expect(language).toHaveValue("pl"));
      expect(reads).toHaveLength(readCount);
      expect(writes).toEqual([original.csrf, colleague.csrf]);
    }
  },
);

it("keeps the saved language when an earlier profile read finishes late", async () => {
  let language = "en";
  let delayProfile = false;
  const deferred = () => {
    let resolve!: () => void;
    const promise = new Promise<void>((done) => {
      resolve = done;
    });
    return { promise, resolve };
  };
  const waiting = deferred();
  const release = deferred();
  const served = deferred();
  const cookie = "language-session";
  server.use(
    http.get("/api/v1/me", async () => {
      const profile = {
        user: { login: "alice", name: null, avatar_url: null, language },
        csrf_token: cookie,
      };
      if (delayProfile) {
        delayProfile = false;
        waiting.resolve();
        await release.promise;
        served.resolve();
      }
      return HttpResponse.json(profile);
    }),
    http.get("/api/v1/settings", () => HttpResponse.json({ ...SETTINGS, language })),
    http.put("/api/v1/settings/language", async ({ request }) => {
      language = ((await request.json()) as { language: string }).language;
      return new HttpResponse(null, { status: 204 });
    }),
  );
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  renderRoute("/settings/llm", queryClient);
  const select = await screen.findByLabelText("Explanations, questions and assessments are written in");
  expect(select).toHaveValue("en");

  delayProfile = true;
  const staleRead = queryClient.refetchQueries({ queryKey: ["me"], exact: true });
  await waiting.promise;
  try {
    await userEvent.selectOptions(select, "pl");
    await waitFor(() => expect(select).toHaveValue("pl"));
  } finally {
    await act(async () => {
      release.resolve();
      await served.promise;
      await staleRead;
    });
  }
  expect(queryClient.getQueryData(["me"])).toMatchObject({ user: { language: "pl" } });
});

it("retries the failed effort without repeating a successful edit from the same Save", async () => {
  const writes: string[] = [];
  let refuse = true;
  server.events.on("request:start", ({ request }) => {
    if (request.method === "PUT") writes.push(new URL(request.url).pathname);
  });
  server.use(
    http.put("/api/v1/settings/llm/ask-effort", () => {
      if (!refuse) return;
      refuse = false;
      return HttpResponse.json({ message: "Try again later.", code: "internal" }, { status: 500 });
    }),
  );
  renderRoute("/settings/llm");
  const model = within(await screen.findByRole("region", { name: "Model and effort" }));
  await userEvent.selectOptions(model.getByLabelText("Model"), "gpt-6-astra");
  await userEvent.selectOptions(model.getByLabelText("Ask effort"), "low");
  await userEvent.click(model.getByRole("button", { name: "Save" }));
  expect(await model.findByRole("alert")).toHaveTextContent("Try again later.");
  expect(model.getByLabelText("Ask effort")).toHaveValue("low");
  await userEvent.click(model.getByRole("button", { name: "Save" }));
  expect(await model.findByText("Saved")).toBeVisible();
  expect(writes).toEqual([
    "/api/v1/settings/llm",
    "/api/v1/settings/llm/ask-effort",
    "/api/v1/settings/llm/ask-effort",
  ]);
});

it("edits an installed-only repository without losing focus or saving other defaults", async () => {
  const saves: { repositories: { repo: { name: string } }[] }[] = [];
  server.events.on("request:start", async ({ request }) => {
    if (request.method === "PUT" && new URL(request.url).pathname === "/api/v1/settings/dashboard")
      saves.push(await request.clone().json());
  });
  renderRoute("/settings/repositories");
  const shortName = await screen.findByLabelText("Short name of acme/live-only");
  await userEvent.type(shortName, "live");
  expect(shortName).toHaveFocus();
  expect(shortName).toHaveValue("LIVE");
  await userEvent.click(screen.getByRole("button", { name: "Save" }));
  await screen.findByText("Dashboard settings saved.");
  expect(saves[0]?.repositories.filter(({ repo }) => repo.name === "live-only")).toEqual([
    expect.objectContaining({ alias: "LIVE" }),
  ]);
});

it("places the GitHub sync save between Slack names and GitHub App", async () => {
  renderRoute("/settings/github-sync");
  const slack = await screen.findByRole("region", { name: "Slack names" });
  const save = screen.getByRole("button", { name: "Save" });
  const github = await screen.findByRole("region", { name: "GitHub App" });
  expect(slack.compareDocumentPosition(save)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
  expect(save.compareDocumentPosition(github)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
});

it("distinguishes loading usage from a reserved route", async () => {
  let resolve!: () => void;
  const promise = new Promise<void>((release) => {
    resolve = release;
  });
  const missing = async () => {
    await promise;
    return new HttpResponse(null, { status: 501 });
  };
  server.use(
    http.get("/api/v1/settings", async () => {
      await promise;
      return HttpResponse.json({ ...SETTINGS, llm_usage: undefined });
    }),
    http.get("/api/v1/settings/ranking-usage", missing),
  );
  try {
    renderRoute("/settings/llm");
    const ranking = within(await screen.findByRole("region", { name: "Ranking today" }));
    const tokens = within(screen.getByRole("region", { name: "Tokens" }));
    expect(ranking.getByRole("status")).toHaveTextContent("Loading ranking usage…");
    expect(tokens.getByText("Today: loading…")).toBeVisible();
    expect(tokens.getByText("Last 7 days: loading…")).toBeVisible();
    expect(tokens.getByText("This month: loading…")).toBeVisible();
    resolve();
    expect(await ranking.findByText("Not available yet")).toBeVisible();
    expect(await tokens.findByText("This month: not available yet")).toBeVisible();
    expect(await tokens.findByText("Today: not available yet")).toBeVisible();
    expect(tokens.getByText("Last 7 days: not available yet")).toBeVisible();
    expect(ranking.queryByRole("alert")).toBeNull();
    expect(tokens.queryByRole("alert")).toBeNull();
  } finally {
    resolve();
  }
});

it("shows zero jobs for usage windows with no token reports", async () => {
  const zero = {
    input_tokens: 0,
    cached_input_tokens: 0,
    output_tokens: 0,
    reasoning_output_tokens: null,
    jobs: 0,
  };
  server.use(
    http.get("/api/v1/settings", () =>
      HttpResponse.json({ ...SETTINGS, llm_usage: { today: null, last_7_days: zero, this_month: zero } }),
    ),
  );
  renderRoute("/settings/llm");
  expect(await screen.findByText("Today: 0 jobs with token reports")).toBeVisible();
  expect(screen.getByText("Last 7 days: 0 jobs with token reports")).toBeVisible();
  expect(await screen.findByText("This month: 0 jobs with token reports")).toBeVisible();
});

it("shows a distinct error for each usage request that fails", async () => {
  server.use(
    http.get("/api/v1/settings", () => new HttpResponse(null, { status: 500 })),
    http.get("/api/v1/settings/ranking-usage", () => new HttpResponse(null, { status: 500 })),
  );
  renderRoute("/settings/llm");
  const ranking = within(await screen.findByRole("region", { name: "Ranking today" }));
  const tokens = within(screen.getByRole("region", { name: "Tokens" }));
  expect(await ranking.findByRole("alert")).toHaveTextContent("Could not load ranking usage");
  expect(await tokens.findByText(/Could not load token usage/)).toHaveRole("alert");
});

it("places the browser and push channel headings under Notifications", async () => {
  renderRoute("/settings/review-loop");
  expect(await screen.findByRole("heading", { name: "Notifications", level: 2 })).toBeVisible();
  for (const name of ["Browser", "ntfy", "Webhook (e.g. Home Assistant)"])
    expect(await screen.findByRole("heading", { name, level: 3 })).toBeVisible();
});

it("shows GitHub webhook losses with the server's counting windows", async () => {
  server.use(
    http.get("/api/v1/settings/github-webhook", () =>
      HttpResponse.json({
        configured: true,
        webhook_url: "https://webhooks.example.test/github",
        last_delivery_at: "2026-11-25T09:00:00Z",
        dropped_deliveries: 7,
        refused_connections: 2,
      }),
    ),
  );
  renderRoute("/settings/github-sync");
  const app = within(await screen.findByRole("region", { name: "GitHub App" }));
  expect(await app.findByText(/Dropped deliveries since last restart, up to 14 days: 7/)).toBeVisible();
  expect(app.getByText(/Connections refused since last restart: 2/)).toBeVisible();
});
