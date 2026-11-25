import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { AssessmentBanner, assessmentText } from "./AssessmentBanner";

const progress = {
  running: true,
  in_pass: 7,
  waiting: 13,
  since: "2026-11-18T10:42:00Z",
  assessed: 0,
  total: 22,
  limited: 2,
  head: "9f3c2e1",
  at: null,
};
const now = new Date("2026-11-18T11:00:00Z");

beforeEach(() => localStorage.clear());

it("keeps the observed counts when an older server cannot distinguish queued and running assessments", () => {
  expect(
    assessmentText({ running: true, assessed: 2, total: 12, limited: 0, head: null, at: null }, now),
  ).toBe("Assessment pending or running · 2 of 12 comments assessed");
});

it("hides the current state across reloads and reappears when progress changes", () => {
  const focusStatus = vi.fn();
  const show = (next = progress, prKey = "acme/node-client/482") => (
    <AssessmentBanner progress={next} head="9f3c2e1" prKey={prKey} now={now} onHide={focusStatus} />
  );
  const first = render(show());
  expect(screen.getByRole("status")).toHaveTextContent(
    "Assessing 7 comments now (since 10:42) · 13 wait for the next pass · 2 not assessed: daily limit",
  );
  fireEvent.click(screen.getByRole("button", { name: "Hide" }));
  expect(focusStatus).toHaveBeenCalledOnce();
  expect(screen.getByRole("status")).toBeEmptyDOMElement();
  first.unmount();

  const { rerender } = render(show());
  expect(screen.getByRole("status")).toBeEmptyDOMElement();
  rerender(show(progress, "acme/node-client/483"));
  expect(screen.getByRole("status")).toBeVisible();
  rerender(show({ ...progress, in_pass: 6 }));
  expect(screen.getByRole("status")).toHaveTextContent("Assessing 6 comments now");
  rerender(show());
  expect(screen.getByRole("status")).toBeVisible();
});

it("uses a waiting count without declaring an active pass", () => {
  render(
    <AssessmentBanner
      progress={{ ...progress, running: false, in_pass: 0, since: null }}
      head="9f3c2e1"
      prKey="acme/node-client/482"
      now={now}
      onHide={vi.fn()}
    />,
  );
  expect(screen.getByRole("status")).toHaveTextContent("13 wait for the next pass");
  expect(screen.getByRole("status")).not.toHaveTextContent("Assessing");
});
