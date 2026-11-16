import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { Tour } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { linesFor, seedFor } from "../../mocks/fixtures";
import { findStep } from "../../screens/tour/sequence";
import { CodeView } from "./CodeView";

const tour = seedFor("default", "tour") as Tour;

function renderStep(stepId: string, state: "default" | "noq" = "default") {
  const t = state === "default" ? tour : (seedFor(state, "tour") as Tour);
  const { step } = findStep(t, stepId)!;
  const { file, side, range } = step.facts;
  const lines = linesFor(state, file, side, range.start_line, range.end_line)!;
  return { step, ...render(<CodeView lines={lines} annotations={step.llm.annotations} />) };
}

describe("CodeView", () => {
  it("renders numbered lines of the range in a labelled, keyboard-scrollable region", () => {
    renderStep("f1.2");
    const region = screen.getByRole("region", { name: "src/rpc/client.rs, lines 88 to 96" });
    expect(region).toHaveAttribute("tabindex", "0");
    const rows = region.querySelectorAll("[data-line]");
    expect([...rows].map((r) => r.getAttribute("data-line"))).toEqual([
      "88",
      "89",
      "90",
      "91",
      "92",
      "93",
      "94",
      "95",
      "96",
    ]);
    expect(region).toHaveTextContent("impl RpcClient {");
  });

  it("puts a neutral gutter mark on exactly the changed lines", () => {
    renderStep("f1.2");
    const marked = [...document.querySelectorAll("[data-changed]")].map((r) => r.getAttribute("data-line"));
    expect(marked).toEqual(["89", "91", "92", "93", "94", "95", "96"]);
    expect(screen.getAllByText("changed line")[0]).toHaveClass("sr-only");
  });

  it("highlights annotated ranges with one highlight whose tooltip states only was → is", async () => {
    const { step } = renderStep("f1.2");
    const highlights = screen.getAllByRole("mark");
    // Lines 91–95 are annotated (two overlapping annotations); 88–90 and 96 are not.
    const covered = highlights.flatMap((h) =>
      [...h.querySelectorAll("[data-line]")].map((r) => r.getAttribute("data-line")),
    );
    expect(covered).toEqual(["91", "92", "93", "94", "95"]);
    const behaviour = step.llm.annotations.find((a) => a.type === "behaviour")!;
    await userEvent.hover(highlights[0]!);
    const tip = screen.getByRole("tooltip");
    expect(tip).toHaveTextContent(`Was: ${behaviour.was}`);
    expect(tip).toHaveTextContent(`Is: ${behaviour.is}`);
    const consequence = step.llm.annotations.find((a) => a.type === "consequence")!;
    expect(tip).not.toHaveTextContent(consequence.text!);
    expectTrustWording(document.body.textContent ?? "");
  });

  it("makes highlights reachable by keyboard", async () => {
    renderStep("f1.5");
    await userEvent.tab();
    await userEvent.tab();
    expect(screen.getByRole("mark")).toHaveFocus();
    expect(screen.getByRole("tooltip")).toBeVisible();
  });

  it("captions removed code read from the base commit", () => {
    renderStep("f4.1");
    const region = screen.getByRole("region", { name: /src\/rpc\/client.rs, lines 52 to 61/ });
    expect(within(region).getByText("Removed code at the base commit")).toBeInTheDocument();
    expect(region.querySelectorAll("[data-changed]")).toHaveLength(10);
  });

  it("shows unannotated changed lines without a highlight", () => {
    renderStep("f1.2", "noq");
    expect(document.querySelectorAll("[data-changed]").length).toBeGreaterThan(0);
  });
});
