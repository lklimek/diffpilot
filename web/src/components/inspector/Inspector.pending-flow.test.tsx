import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import type { Tour } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { lazyTour } from "../../mocks/lazy-flows";
import { Inspector } from "./Inspector";

it("a flow not analysed yet shows its analysis state, not a risk label, and lists each file once", () => {
  const tour = lazyTour(seedFor("default", "tour") as Tour);
  tour.analysis.state = { state: "paused", reason: "user", resumes_at: null, done: 1, total: 5 };
  const flow = tour.flows[1]!;
  flow.facts.files = ["src/a.rs", "src/a.rs", "src/b.rs"];
  render(<Inspector tour={tour} block={{ block: "flow_intro", flow_id: flow.id }} />);
  expect(screen.getByText("Not analysed yet — analysis paused")).toBeVisible();
  expect(screen.queryByText("not rated")).toBeNull();
  expect(screen.getAllByText("src/a.rs")).toHaveLength(1);
});
