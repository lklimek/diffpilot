// Times of server events in the browser's time zone (UI English, 24-hour clock).
const CLOCK = new Intl.DateTimeFormat("en-GB", { hour: "2-digit", minute: "2-digit" });
const DAY_CLOCK = new Intl.DateTimeFormat("en-GB", {
  weekday: "short",
  day: "numeric",
  month: "short",
  hour: "2-digit",
  minute: "2-digit",
});
const DATE_TIME = new Intl.DateTimeFormat("en-GB", {
  day: "numeric",
  month: "short",
  year: "numeric",
  hour: "2-digit",
  minute: "2-digit",
});

/** `HH:MM` when `iso` is on the day of `now`, else with the day (`Wed 18 Nov, 08:05`). */
export function clockText(iso: string, now = new Date()): string {
  const time = new Date(iso);
  return time.toDateString() === now.toDateString() ? CLOCK.format(time) : DAY_CLOCK.format(time);
}

/** Date and time (`16 Nov 2026, 09:31`). */
export const dateTimeText = (iso: string): string => DATE_TIME.format(new Date(iso));
