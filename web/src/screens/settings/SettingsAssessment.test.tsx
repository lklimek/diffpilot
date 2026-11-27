import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HttpResponse, http } from "msw";
import { expect, it } from "vitest";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { unlinkedCodex, assessmentSettings } from "../../mocks/assessment";

const server = startMockServer();
it("shows a code, polls only the local endpoint, and handles quarantine without restarting login", async () => {
  let posts = 0;
  server.use(
    http.post("/api/v1/settings/codex/link", async ({ request }) => {
      posts++;
      expect(await request.json()).toEqual({ mode: "chatgpt_plan" });
      expect(request.headers.get("X-CSRF-Token")).toBeTruthy();
      return HttpResponse.json({
        user_code: "FAKE-ONLY",
        verification_url: "https://example.invalid/device",
        expires_at: new Date(Date.now() + 300_000).toISOString(),
      });
    }),
    http.get("/api/v1/settings/codex/link", () =>
      HttpResponse.json(posts ? { ...unlinkedCodex, relink_reason: "quarantined" } : unlinkedCodex),
    ),
  );
  renderRoute("/settings");
  const card = await screen.findByRole("region", { name: "Codex account" });
  await userEvent.click(await within(card).findByRole("button", { name: "Link Codex account" }));
  expect(await within(card).findByText(/outcome is uncertain/)).toBeVisible();
  await waitFor(() =>
    expect(within(card).getByRole("button", { name: "Relink Codex account" })).not.toHaveAttribute(
      "aria-disabled",
    ),
  );
  expect(posts).toBe(1);
});
it("reports save failures, preserves selections and does not queue work", async () => {
  const bodies: unknown[] = [];
  server.use(
    http.get("/api/v1/settings/comment-assessment", () =>
      HttpResponse.json({
        ...assessmentSettings,
        config: { ...assessmentSettings.config, ab_prs: ["acme/node-client#482"] },
      }),
    ),
    http.put("/api/v1/settings/comment-assessment", async ({ request }) => {
      bodies.push(await request.json());
      return HttpResponse.json(
        { code: "invalid_request", message: "Settings could not be saved." },
        { status: 400 },
      );
    }),
  );
  renderRoute("/settings/review-loop");
  await screen.findByLabelText("Primary assessment method");
  const card = screen.getByRole("region", { name: "Comment assessment" });
  await userEvent.selectOptions(await within(card).findByLabelText("Primary assessment method"), "prompt_v4");
  await userEvent.click(within(card).getByRole("button", { name: "Save assessment settings" }));
  expect(await within(card).findByRole("alert")).toHaveTextContent("Settings could not be saved.");
  expect(bodies).toEqual([
    {
      config: { primary_method: "prompt_v4", ab_prs: ["acme/node-client#482"], ab_until: null },
      revision: assessmentSettings.revision,
    },
  ]);
});
it("a missing comparison is empty", async () => {
  server.use(
    http.get("/api/v1/prs/:owner/:name/:number/comments/assessment-comparison", () =>
      HttpResponse.json({ code: "not_found", message: "Not found" }, { status: 404 }),
    ),
  );
  renderRoute("/pr/acme/node-client/482/comments");
  await userEvent.click(await screen.findByText("Assessment comparison (read-only)"));
  expect(await screen.findByText("No assessment comparison yet.")).toBeVisible();
});

it.each([401, 403])("comparison authentication failure %s stays an error", async (status) => {
  server.use(
    http.get("/api/v1/prs/:owner/:name/:number/comments/assessment-comparison", () =>
      HttpResponse.json({ code: "forbidden", message: "Sign in to read this comparison." }, { status }),
    ),
  );
  renderRoute("/pr/acme/node-client/482/comments");
  await userEvent.click(await screen.findByText("Assessment comparison (read-only)"));
  expect(await screen.findByText("Sign in to read this comparison.")).toBeVisible();
  expect(screen.queryByText("No assessment comparison yet.")).toBeNull();
});
it("restores an in-flight code from GET after remount without posting", async () => {
  let posts = 0;
  const linking = { ...unlinkedCodex, state: "linking", relink_reason: null };
  server.use(
    http.get("/api/v1/settings/codex", () => HttpResponse.json(linking)),
    http.get("/api/v1/settings/codex/link", () =>
      HttpResponse.json({
        ...linking,
        challenge: {
          user_code: "RESTORED",
          verification_url: "https://example.invalid/device",
          expires_at: new Date(Date.now() + 300000).toISOString(),
        },
      }),
    ),
    http.post("/api/v1/settings/codex/link", () => {
      posts++;
      return HttpResponse.error();
    }),
  );
  renderRoute("/settings");
  expect(await screen.findByText("RESTORED")).toBeVisible();
  cleanup();
  renderRoute("/settings");
  expect(await screen.findByText("RESTORED")).toBeVisible();
  expect(posts).toBe(0);
});
it("conflicting settings refresh the revision without automatically replaying a stale write", async () => {
  let gets = 0;
  const bodies: unknown[] = [];
  server.use(
    http.get("/api/v1/settings/comment-assessment", () =>
      HttpResponse.json({ ...assessmentSettings, revision: gets++ ? "newer" : "initial" }),
    ),
    http.put("/api/v1/settings/comment-assessment", async ({ request }) => {
      bodies.push(await request.json());
      return HttpResponse.json(
        { code: "conflict", message: "Settings changed. Reload and try again." },
        { status: 409 },
      );
    }),
  );
  renderRoute("/settings/review-loop");
  await screen.findByLabelText("Primary assessment method");
  const card = screen.getByRole("region", { name: "Comment assessment" });
  await userEvent.selectOptions(await within(card).findByLabelText("Primary assessment method"), "prompt_v4");
  await userEvent.click(within(card).getByRole("button", { name: "Save assessment settings" }));
  expect(await within(card).findByRole("alert")).toHaveTextContent("Your edits are kept.");
  await waitFor(() => expect(gets).toBe(2));
  expect(bodies).toEqual([
    { config: { ...assessmentSettings.config, primary_method: "prompt_v4" }, revision: "initial" },
  ]);
});

it("repeated link-status reads after unlink never create another connection", async () => {
  const { api } = await import("../../api/client");
  expect((await api.getCodexLink()).state).toBe("relink_needed");
  await api.linkCodex();
  expect((await api.getCodexLink()).state).toBe("linking");
  expect((await api.getCodexLink()).state).toBe("linked");
  await api.unlinkCodex();
  expect((await api.getCodexLink()).state).toBe("relink_needed");
  expect((await api.getCodexLink()).state).toBe("relink_needed");
});

it("keeps edits through a conflicting primary and deadline change and reapplies them to the new revision", async () => {
  const changed = {
    ...assessmentSettings,
    revision: "elsewhere",
    config: {
      primary_method: "prompt_v4" as const,
      ab_until: "2026-12-01T00:00:00Z",
      ab_prs: ["acme/other#10"],
    },
  };
  let current = assessmentSettings;
  const bodies: unknown[] = [];
  server.use(
    http.get("/api/v1/settings/comment-assessment", () => HttpResponse.json(current)),
    http.put("/api/v1/settings/comment-assessment", async ({ request }) => {
      const body = await request.json();
      bodies.push(body);
      if (bodies.length === 1) {
        current = changed;
        return HttpResponse.json({ code: "conflict", message: "Reload and try again." }, { status: 409 });
      }
      if (bodies.length === 3)
        return HttpResponse.json(
          { code: "invalid_request", message: "Settings could not be saved." },
          { status: 400 },
        );
      current = { ...changed, revision: "saved", config: (body as typeof changed).config };
      return HttpResponse.json(current);
    }),
  );
  renderRoute("/settings/review-loop");
  await screen.findByLabelText("Primary assessment method");
  const card = screen.getByRole("region", { name: "Comment assessment" });
  const primary = await within(card).findByLabelText("Primary assessment method");
  const deadline = within(card).getByLabelText("Optional A/B deadline (UTC)");
  await userEvent.selectOptions(primary, "prompt_v4");
  await userEvent.type(deadline, "2026-11-30T00:00:00Z");
  await userEvent.click(within(card).getByRole("button", { name: "Save assessment settings" }));
  expect(await within(card).findByRole("alert")).toHaveTextContent("Your edits are kept.");
  expect(within(card).queryByText(/Reload/)).toBeNull();
  expect(await within(card).findByText(/Primary method changed elsewhere:.*prompt_v4/)).toBeVisible();
  expect(within(card).getByText(/A\/B deadline changed elsewhere:.*2026-12-01T00:00:00Z/)).toBeVisible();
  expect(within(card).getByText(/Selected PRs changed elsewhere:.*acme\/other#10/)).toBeVisible();
  expect(primary).toHaveValue("prompt_v4");
  expect(deadline).toHaveValue("2026-11-30T00:00:00Z");
  expect(bodies).toHaveLength(1);
  await userEvent.click(within(card).getByRole("button", { name: "Save assessment settings" }));
  expect(await screen.findByText("Assessment settings saved.")).toBeVisible();
  const savedCard = screen.getByRole("region", { name: "Comment assessment" });
  expect(bodies[1]).toEqual({
    revision: "elsewhere",
    config: { ...changed.config, ab_until: "2026-11-30T00:00:00.000Z" },
  });
  expect(within(savedCard).getByLabelText("Optional A/B deadline (UTC)")).toHaveValue(
    "2026-11-30T00:00:00.000Z",
  );
  expect(within(savedCard).queryByText(/changed elsewhere/)).toBeNull();
  const nextDeadline = within(savedCard).getByLabelText("Optional A/B deadline (UTC)");
  await userEvent.clear(nextDeadline);
  await userEvent.type(nextDeadline, "2026-11-29T00:00:00Z");
  await userEvent.click(within(savedCard).getByRole("button", { name: "Save assessment settings" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Settings could not be saved.");
  expect(screen.getByLabelText("Optional A/B deadline (UTC)")).toHaveValue("2026-11-29T00:00:00Z");
});
