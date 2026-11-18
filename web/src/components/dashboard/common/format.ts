// Number and time texts of the dashboard (UI English; scores arrive in tenths of a point).
import type { ItemScore, Tier } from "../../../api/generated/types";

/** `+4`, `−3` (minus sign), `0`. */
export const signed = (n: number): string => (n > 0 ? `+${n}` : n < 0 ? `−${Math.abs(n)}` : "0");

/** Points of a part from tenths: `+12.7`, `−20`, `0`. */
export function pointsText(tenths: number): string {
  const text = (Math.abs(tenths) / 10).toFixed(1).replace(/\.0$/, "");
  return tenths > 0 ? `+${text}` : tenths < 0 ? `−${text}` : "0";
}

/** Whole points of a score, `~` first while it is not final (`~82`). */
export const scoreText = (score: ItemScore): string =>
  `${score.state === "final" ? "" : "~"}${Math.round(score.total_tenths / 10)}`;

/** Upper-case tier name (`P1`). */
export const tierName = (tier: Tier): string => tier.toUpperCase();

/** The rule of a tier, as shown in its group header. */
export const TIER_RULE: Record<Exclude<Tier, "p0">, string> = {
  p1: "score ≥ 65",
  p2: "score 35–<65",
  p3: "score < 35",
};

/** Whole days between `iso` and `now` (never negative). */
export const daysSince = (iso: string, now: Date): number =>
  Math.max(0, Math.floor((now.getTime() - new Date(iso).getTime()) / 86_400_000));

/** Short duration: `45 s`, `1 min 38 s`, `2 h 15 min`. */
export function durationText(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s} s`;
  const min = Math.floor(s / 60);
  if (min < 60) return s % 60 ? `${min} min ${s % 60} s` : `${min} min`;
  return min % 60 ? `${Math.floor(min / 60)} h ${min % 60} min` : `${Math.floor(min / 60)} h`;
}

/** `just now`, `5 min ago`, `2 h 15 min ago`, `3 d ago`. */
export function agoText(iso: string, now: Date): string {
  const ms = now.getTime() - new Date(iso).getTime();
  if (ms < 60_000) return "just now";
  if (ms >= 86_400_000) return `${Math.floor(ms / 86_400_000)} d ago`;
  return `${durationText(Math.floor(ms / 60_000) * 60_000)} ago`;
}

/** `m:ss` of an elapsed time. */
export function elapsedText(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

const DAY = new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "short" });

/** Day of a judgement or event (`17 Nov`). */
export const dayText = (iso: string): string => DAY.format(new Date(iso));
