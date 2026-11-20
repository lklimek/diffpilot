import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HttpResponse, http } from "msw";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { expectTrustWording } from "../../__tests__/wording";
import type { AnalysisSummary } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { setScenario } from "../../mocks/scenarios";

const server = startMockServer();
const ID = (seedFor("default", "analysis") as AnalysisSummary).id;

beforeEach(() => {
  FakeEventSource.reset();
  vi.stubGlobal("EventSource", FakeEventSource);
});
afterEach(() => {
  vi.unstubAllGlobals();
  window.localStorage.clear();
});

const heading = () => screen.findByRole("heading", { level: 1, name: "Dependencies" });
const repositories = () => screen.findByRole("list", { name: "Repositories" });

describe("Dependencies view (brief 2026-11-20, plan U1–U3)", () => {
  it("is the third view of the review: the menu names it and the tree opens with this repository", async () => {
    renderRoute(`/analyses/${ID}/deps`);
    await heading();
    expect(screen.getByRole("button", { name: "View: Dependencies" })).toBeVisible();
    const list = await repositories();
    expect(within(list).getByRole("button", { name: /^acme\/node-client/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    expect(within(list).getByRole("button", { name: /^node-client/ })).toHaveTextContent("212 lines changed");
    expectTrustWording(document.body.textContent ?? "");
  });

  it("the Files view's menu leads here", async () => {
    const router = renderRoute(`/analyses/${ID}/files`);
    await userEvent.click(await screen.findByRole("button", { name: "View: Files" }));
    await userEvent.click(screen.getByRole("link", { name: "Dependencies" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/analyses/${ID}/deps`));
    await heading();
  });

  it("a fact's line in a file of the pull request opens the Files view at that line", async () => {
    setScenario("deps-multi");
    const router = renderRoute(`/analyses/${ID}/deps`);
    await repositories();
    await userEvent.click(screen.getByRole("link", { name: "src/rpc/error.rs:12" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/analyses/${ID}/files`));
    expect(decodeURIComponent(router.state.location.hash)).toBe(
      "#lines?file=src/rpc/error.rs&side=head&line=12",
    );
  });

  it("a line in a file the pull request does not change opens on GitHub at the head commit", async () => {
    setScenario("deps-multi");
    renderRoute(`/analyses/${ID}/deps`);
    await repositories();
    const tour = seedFor("default", "tour") as { analysis: { pr: { head_sha: string } } };
    const def = screen.getByRole("link", { name: "crates/node-proto/src/event.rs:8" });
    expect(def).toHaveAttribute(
      "href",
      `https://github.com/acme/node-client/blob/${tour.analysis.pr.head_sha}/crates/node-proto/src/event.rs#L8`,
    );
    expect(def).toHaveAttribute("target", "_blank");
  });

  it("a line of a PR file the Files view does not render (generated Cargo.lock) opens on GitHub", async () => {
    setScenario("deps-multi");
    renderRoute(`/analyses/${ID}/deps`);
    const list = await repositories();
    await userEvent.click(within(list).getByRole("button", { name: /^acme\/node-sdk/ }));
    await userEvent.click(within(list).getByRole("button", { name: /^rpc-types/ }));
    const tour = seedFor("default", "tour") as { analysis: { pr: { head_sha: string } } };
    const lock = screen.getByRole("link", { name: "Cargo.lock:30" });
    expect(lock).toHaveAttribute(
      "href",
      `https://github.com/acme/node-client/blob/${tour.analysis.pr.head_sha}/Cargo.lock#L30`,
    );
    expect(lock).toHaveAttribute("target", "_blank");
    expect(lock).toHaveAccessibleDescription(/on GitHub at the analysed commit \(new tab\)/);
  });

  it("a flow link opens the flow in the Flows view", async () => {
    const router = renderRoute(`/analyses/${ID}/deps`);
    await repositories();
    await userEvent.click(screen.getByRole("link", { name: "Send an RPC request with retry" }));
    await waitFor(() => expect(router.state.location.hash).toBe("#f1"));
    expect(router.state.location.pathname).toBe(`/analyses/${ID}`);
  });

  it("remembers opened and closed levels per analysis", async () => {
    setScenario("deps-multi");
    renderRoute(`/analyses/${ID}/deps`);
    const list = await repositories();
    await userEvent.click(within(list).getByRole("button", { name: /^acme\/node-sdk/ }));
    await userEvent.click(within(list).getByRole("button", { name: /^acme\/node-client/ }));
    expect(JSON.parse(localStorage.getItem(`diffpilot.deps.open.${ID}`)!)).toEqual({
      "github:acme/node-sdk": true,
      this: false,
    });
    document.body.innerHTML = "";
    renderRoute(`/analyses/${ID}/deps`);
    const again = await repositories();
    expect(within(again).getByRole("button", { name: /^acme\/node-sdk/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    expect(within(again).getByRole("button", { name: /^acme\/node-client/ })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
  });

  it("not_rust: the sentence only, no tree", async () => {
    setScenario("deps-not-rust");
    renderRoute(`/analyses/${ID}/deps`);
    expect(
      await screen.findByText(
        "Dependencies: not available — this view reads Cargo (Rust) workspaces only; nothing was checked.",
      ),
    ).toBeVisible();
    expect(screen.queryByRole("list", { name: "Repositories" })).toBeNull();
  });

  it("a read error says so and Retry reads again (nothing is shown as checked)", async () => {
    setScenario("deps-error");
    renderRoute(`/analyses/${ID}/deps`);
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The dependencies could not be read: The repository could not be read. Try again in a minute.",
    );
    setScenario("deps-multi");
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await repositories();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("an analysis not finished goes to its progress page", async () => {
    server.use(
      http.get("/api/v1/analyses/:id/dependencies", () =>
        HttpResponse.json({ code: "analysis_not_finished", message: "Not finished." }, { status: 409 }),
      ),
    );
    const router = renderRoute(`/analyses/${ID}/deps`);
    await waitFor(() => expect(router.state.location.pathname).toBe(`/analyses/${ID}/progress`));
  });

  it("controlled organisations saved in Settings apply to the view at once", async () => {
    setScenario("deps-no-controlled");
    renderRoute("/settings");
    const field = await screen.findByLabelText("Add a controlled organisation");
    await userEvent.type(field, "dashpay{Enter}");
    const card = screen.getByRole("region", { name: "Controlled organisations" });
    await userEvent.click(within(card).getByRole("button", { name: "Save organisations" }));
    expect(await within(card).findByRole("status")).toHaveTextContent("Controlled organisations saved.");
    expect(card).toHaveTextContent(/changed by lklimek at/);
    document.body.innerHTML = "";
    renderRoute(`/analyses/${ID}/deps`);
    await repositories();
    expect(document.body).toHaveTextContent("Yours: acme (this pull request's owner), dashpay.");
    const platform = screen.getByRole("button", { name: /^dashpay\/platform/ }).closest("li")!;
    expect(within(platform).getByText("yours")).toBeVisible();
  });
});
