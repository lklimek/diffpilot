import { cleanup, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HttpResponse, http } from "msw";
import { expect, it } from "vitest";
import itemsJson from "../../../../fixtures/dashboard/api/items.json";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import type { ItemPage, Tour, WorkItem } from "../../api/generated/types";
import { mockFiles } from "../../mocks/files";
import { seedFor } from "../../mocks/fixtures";

const server = startMockServer();

it("links paths in the visible ranking before opening details and keeps unknown paths as text", async () => {
  const page = itemsJson as unknown as ItemPage;
  const original = page.items.find((item) => item.id === "NC-482")!;
  const review = { ...original.review!, head_moved: false };
  const item: WorkItem = {
    ...original,
    review,
    llm: {
      ...original.llm!,
      why: "Check src/rpc/client.rs:92-93 and missing.rs.",
      next: "Look at `./src/rpc/client.rs#L96` and src/rpc/retry.rs#L4.",
    },
  };
  const files = mockFiles(seedFor("default", "tour") as Tour);
  const reads: string[] = [];
  server.use(
    http.get("/api/v1/dashboard/items", () => HttpResponse.json({ ...page, items: [item] })),
    http.get("/api/v1/analyses/:id/files", ({ params }) => {
      reads.push(String(params.id));
      return HttpResponse.json({ ...files, current_head: review.analysed_head });
    }),
  );

  renderRoute("/");
  const details = await screen.findByRole("button", { name: "Details of NC-482" });
  expect(details).toHaveAttribute("aria-expanded", "false");
  const path = `/pr/acme/node-client/482/analyses/${review.analysis_id}/files`;
  expect(await screen.findByRole("link", { name: "src/rpc/client.rs:92-93" })).toHaveAttribute(
    "href",
    `${path}#lines?file=src%2Frpc%2Fclient.rs&side=head&line=92&end=93`,
  );
  expect(screen.getByRole("link", { name: "./src/rpc/client.rs#L96" })).toHaveAttribute(
    "href",
    `${path}#lines?file=src%2Frpc%2Fclient.rs&side=head&line=96`,
  );
  expect(screen.getByRole("link", { name: "src/rpc/retry.rs#L4" })).toHaveAttribute(
    "href",
    `${path}#lines?file=src%2Frpc%2Fretry.rs&side=head&line=4`,
  );
  expect(screen.queryByRole("link", { name: "missing.rs" })).toBeNull();
  expect(reads).toEqual([review.analysis_id]);

  await userEvent.click(details);
  await userEvent.click(details);
  expect(screen.getByRole("link", { name: "src/rpc/client.rs:92-93" })).toBeVisible();
  expect(reads).toEqual([review.analysis_id]);

  cleanup();
  server.use(
    http.get("/api/v1/dashboard/items", () =>
      HttpResponse.json({ ...page, items: [{ ...item, review: { ...review, head_moved: true } }] }),
    ),
  );
  renderRoute("/");
  await userEvent.click(await screen.findByRole("button", { name: "Details of NC-482" }));
  expect(screen.queryByRole("link", { name: "src/rpc/client.rs:92-93" })).toBeNull();
  expect(reads).toEqual([review.analysis_id]);
});

it.each([
  { why: "The RPC API needs review.", file: null },
  { why: "Check the Makefile.", file: "Makefile" },
  { why: "Check `BOOTSTRAP`.", file: "BOOTSTRAP" },
])("fetches a Files index only when the visible ranking could name a file ($why)", async ({ why, file }) => {
  const page = itemsJson as unknown as ItemPage;
  const original = page.items.find((item) => item.id === "NC-482")!;
  const item = {
    ...original,
    review: { ...original.review!, head_moved: false },
    llm: { ...original.llm!, why, next: "Check the timeout.", alarm: null },
  };
  const files = mockFiles(seedFor("default", "tour") as Tour);
  const reads: string[] = [];
  server.use(
    http.get("/api/v1/dashboard/items", () => HttpResponse.json({ ...page, items: [item] })),
    http.get("/api/v1/analyses/:id/files", ({ params }) => {
      reads.push(String(params.id));
      return HttpResponse.json({
        ...files,
        current_head: item.review.analysed_head,
        files: file ? [{ ...files.files[0]!, path: file }] : files.files,
      });
    }),
  );
  renderRoute("/");
  if (file) {
    expect(await screen.findByRole("link", { name: file })).toBeVisible();
  } else {
    expect(await screen.findByText(why)).toBeVisible();
  }
  expect(reads).toEqual(file ? [item.review.analysis_id] : []);
});
