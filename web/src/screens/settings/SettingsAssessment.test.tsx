import { screen, waitFor, within } from "@testing-library/react";
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
      HttpResponse.json({ ...unlinkedCodex, relink_reason: "quarantined" }),
    ),
  );
  renderRoute("/settings");
  const card = await screen.findByRole("region", { name: "Codex account" });
  await userEvent.click(await within(card).findByRole("button", { name: "Link Codex account" }));
  expect(await within(card).findByText(/outcome is uncertain/)).toBeVisible();
  await waitFor(() =>
    expect(within(card).getByRole("button", { name: "Relink Codex account" })).toBeEnabled(),
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
  expect(bodies).toEqual([{ primary_method: "prompt_v4", ab_prs: ["acme/node-client#482"], ab_until: null }]);
});
it("a missing comparison is empty, while authentication errors remain errors", async () => {
  server.use(
    http.get("/api/v1/prs/:owner/:name/:number/comments/assessment-comparison", () =>
      HttpResponse.json({ code: "not_found", message: "Not found" }, { status: 404 }),
    ),
  );
  renderRoute("/pr/acme/node-client/482/comments");
  await userEvent.click(await screen.findByText("Assessment comparison (read-only)"));
  expect(await screen.findByText("No assessment comparison yet.")).toBeVisible();
});
