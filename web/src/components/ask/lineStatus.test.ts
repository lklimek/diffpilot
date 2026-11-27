import { expect, it } from "vitest";
import type { AskQuestion, Tour } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { indexAskLineStatus } from "./lineStatus";

const tour = seedFor("default", "tour") as Tour;
const answered: AskQuestion = {
  id: "a",
  anchor: { kind: "step", step_id: "f1.2", lines: { start_line: 89, end_line: 90 } },
  text: "About these lines",
  mine: true,
  state: "answered",
  answer: { answer: "An answer", evidence: [] },
  failure: null,
  created_at: "2026-11-17T10:00:00Z",
  answered_at: "2026-11-17T10:00:01Z",
};

it("aggregates finished and pending threads only on their exact file-side line", () => {
  const followUp: AskQuestion = { ...answered, id: "b", parent_id: "a", answered_at: null };
  const failed: AskQuestion = {
    ...answered,
    id: "c",
    anchor: {
      kind: "lines",
      path: "src/rpc/client.rs",
      side: "base",
      range: { start_line: 90, end_line: 90 },
    },
    state: "failed",
    answer: null,
    failure: "Worker failed",
    answered_at: null,
  };
  const status = indexAskLineStatus(tour, [answered, followUp, failed])!;
  expect(status("src/rpc/client.rs", "head", 89)).toBeUndefined();
  expect(status("src/rpc/client.rs", "head", 90)).toBe("running");
  expect(status("src/rpc/client.rs", "base", 90)).toBe("failed");
  expect(status("src/rpc/wrong.rs", "head", 90)).toBeUndefined();
  expect(indexAskLineStatus(tour, [answered, failed])!("src/rpc/client.rs", "head", 90)).toBe("answered");
});

it("does not mark whole subjects, unavailable steps or questions from another reader", () => {
  expect(
    indexAskLineStatus(tour, [
      { ...answered, mine: false },
      { ...answered, anchor: { kind: "step", step_id: "f1.2", lines: null } },
      { ...answered, anchor: { kind: "lines", path: "Cargo.toml", side: "head", range: null } },
      { ...answered, anchor: { kind: "step", step_id: "gone", lines: { start_line: 1, end_line: 1 } } },
    ]),
  ).toBeUndefined();
});
