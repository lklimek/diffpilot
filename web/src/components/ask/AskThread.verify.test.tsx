// The whole conversation names a lines question by its file and lines, whatever filter key was last chosen.
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { AskQuestion } from "../../api/generated/types";
import { AskThread } from "./AskThread";

const linesQuestion: AskQuestion = {
  id: "q1",
  mine: true,
  asked_by: "alice",
  anchor: { kind: "lines", path: "src/a.rs", side: "head", range: { start_line: 3, end_line: 9 } },
  text: "Why?",
  state: "answered",
  answer: { answer: "Because.", evidence: [] },
  failure: null,
  created_at: "2026-11-16T10:20:00Z",
};

describe("AskThread with a filter key no longer offered", () => {
  it("shows the whole conversation and names a lines question's file and lines", () => {
    // Files view: "This block" was chosen, then the reader went back to the file (no block filter any more).
    render(
      <AskThread
        questions={[linesQuestion]}
        scope="block"
        onScope={() => {}}
        placeOf={() => null}
        placeOfQuestion={() => null}
        filters={[]}
        isCurrent={() => false}
      />,
    );
    expect(screen.getByText("On src/a.rs:3–9")).toBeInTheDocument();
  });
});
