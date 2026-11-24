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
  renderRoute("/settings");
  const card = await screen.findByRole("region", { name: "Comment assessment" });
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
  renderRoute("/settings");
  const card = await screen.findByRole("region", { name: "Comment assessment" });
  await userEvent.selectOptions(await within(card).findByLabelText("Primary assessment method"), "prompt_v4");
  await userEvent.click(within(card).getByRole("button", { name: "Save assessment settings" }));
  expect(await within(card).findByRole("alert")).toHaveTextContent("Settings changed. Reload and try again.");
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
