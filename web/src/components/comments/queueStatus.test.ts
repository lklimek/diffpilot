import { describe, expect, it } from "vitest";
import fixture from "../../../../docs/contracts/fixtures/needs-you-cases.json";
import type { CommentItem, QueueEntry } from "../../api/generated/types";
import { commentsByNo, needsYouQueue } from "./queueStatus";

const items = fixture.items.map(
  (item, index) =>
    ({ ...fixture.defaults, ...item, status: fixture.expected_statuses[index] }) as CommentItem,
);
const queue = fixture.queue as QueueEntry[];

describe("Needs you fixture", () => {
  it("uses the server status for queue membership even when the item's facts disagree", () => {
    const item = { ...items[0]!, status: "needs_you" as const };
    const queue = [{ item_nos: [item.no], reason: "single" as const }];
    expect(needsYouQueue(queue, commentsByNo([item])!)).toEqual(queue);
  });

  it("counts only queue members, including gates", () => {
    const byNo = commentsByNo(items)!;
    const numbers = needsYouQueue(queue, byNo).flatMap((entry) => entry.item_nos);
    expect(numbers).toHaveLength(fixture.expected_needs_you);
    expect(numbers.filter((no) => byNo.get(no)!.gates.length > 0)).toHaveLength(
      fixture.expected_needs_you_gates,
    );
  });
});
