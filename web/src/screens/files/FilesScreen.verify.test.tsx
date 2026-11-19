// Verification checks of fix round 1 (proposal §2, §6; http-api.md Ranges).
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import type { AnalysisSummary } from "../../api/generated/types";
import { parseFilesHash } from "../../components/files/model";
import { seedFor } from "../../mocks/fixtures";

startMockServer();
const ID = (seedFor("default", "analysis") as AnalysisSummary).id;

beforeEach(() => {
  FakeEventSource.reset();
  vi.stubGlobal("EventSource", FakeEventSource);
});
afterEach(() => {
  vi.unstubAllGlobals();
  window.localStorage.clear();
});

const inspector = () => screen.getByRole("complementary", { name: "Inspector" });
const inspectorHeading = () =>
  within(inspector())
    .getAllByRole("heading", { level: 2 })
    .find((h) => !h.hasAttribute("data-column-title"))!;
const fileHeading = () =>
  within(screen.getByRole("main"))
    .getAllByRole("heading", { level: 2 })
    .find((h) => !h.hasAttribute("data-column-title"))!;

async function open(hash: string) {
  const router = renderRoute(`/analyses/${ID}/files${hash}`);
  await waitFor(() => expect(fileHeading()).toBeDefined());
  return router;
}

describe("Files view fix round 1", () => {
  it("a link never spans more than one range request of lines", () => {
    const link = parseFilesHash("#lines?file=a.rs&side=head&line=1&end=9007199254740991");
    expect(link!.end! - link!.line! + 1).toBeLessThanOrEqual(2000);
  });

  it("Shift+click after a linked selection extends it", async () => {
    const router = await open("#lines?file=src%2Frpc%2Fclient.rs&side=head&line=100&end=102");
    const u = userEvent.setup();
    await u.keyboard("{Shift>}");
    await u.click(await within(screen.getByRole("main")).findByRole("button", { name: "Select line 105" }));
    await u.keyboard("{/Shift}");
    expect(decodeURIComponent(router.state.location.hash)).toBe(
      "#lines?file=src/rpc/client.rs&side=head&line=100&end=105",
    );
  });

  it("c on a pure-deletion hunk opens the comment composer on its removed line", async () => {
    await open("#lines?file=src%2Frpc%2Fclient.rs&side=base&line=52&end=62");
    expect(inspectorHeading()).toHaveTextContent("client.rs · hunk, removed lines 52–62");
    await within(screen.getByRole("main")).findAllByRole("button", { name: /^Select removed line 52$/ });
    await userEvent.setup().keyboard("c");
    await waitFor(() =>
      expect(
        within(inspector()).getByRole("textbox", { name: /^Comment on src\/rpc\/client\.rs/ }),
      ).toHaveFocus(),
    );
  });
});
