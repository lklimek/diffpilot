import { describe, expect, it } from "vitest";
import { clockText, dateTimeText } from "./time";

// Local dates, so the expectations hold in every time zone.
const local = (day: number, hour: number, minute: number) => new Date(2026, 10, day, hour, minute);

describe("time formatting", () => {
  it("clockText: HH:MM today, with the day otherwise", () => {
    const now = local(17, 9, 0);
    expect(clockText(local(17, 14, 20).toISOString(), now)).toBe("14:20");
    expect(clockText(local(18, 8, 5).toISOString(), now)).toBe("Wed 18 Nov, 08:05");
  });

  it("dateTimeText: date and time", () => {
    expect(dateTimeText(local(16, 9, 31).toISOString())).toBe("16 Nov 2026, 09:31");
  });
});
