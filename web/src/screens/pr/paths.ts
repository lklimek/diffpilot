// In-app paths of the PR page and the Comments view (plan docs/review-workflow-plan.md §3 V0–V6).
import type { PrRef } from "../../api/generated/types";
import type { CommentsPage } from "../../components/comments/CommentsTabs";

const seg = (value: string | number) => encodeURIComponent(String(value));

/** V0 PR page: `/pr/:owner/:repo/:number`. */
export const prPath = (pr: PrRef) => `/pr/${seg(pr.repo.owner)}/${seg(pr.repo.name)}/${seg(pr.number)}`;

const PAGE_PATH: Record<CommentsPage, string> = {
  questions: "",
  table: "/table",
  rounds: "/rounds",
  "carry-overs": "/carry-overs",
  log: "/log",
};

/** A Comments page of the PR (V1–V5). */
export const commentsPath = (pr: PrRef, page: CommentsPage = "questions") =>
  `${prPath(pr)}/comments${PAGE_PATH[page]}`;

/** Questions with one item open (`?item=C14`). */
export const itemPath = (pr: PrRef, no: number) => `${commentsPath(pr)}?item=C${no}`;

/** The exact GitHub review thread, resolved within the PR's Comments view. */
export const githubThreadPath = (pr: PrRef, id: string) =>
  `${commentsPath(pr)}?${new URLSearchParams({ thread: id })}`;

/** Carry-overs of all pull requests (V5). */
export const ALL_CARRY_OVERS_PATH = "/comments/carry-overs";

/** The PR of the route parameters `owner`, `repo`, `number`. */
export const prOfParams = (params: { owner?: string; repo?: string; number?: string }): PrRef => ({
  repo: { owner: params.owner ?? "", name: params.repo ?? "" },
  number: Number(params.number ?? 0),
});

/** GitHub pull links and owner/repo#number from the PR opener. */
export function parsePrRef(input: string): PrRef | null {
  const text = input.trim();
  let match = /^([a-z\d-]{1,39})\/([a-z\d_.-]{1,100})#([1-9]\d*)$/i.exec(text);
  if (!match) {
    try {
      const url = new URL(text.includes("://") ? text : `https://${text}`);
      if (
        (url.protocol !== "https:" && url.protocol !== "http:") ||
        (url.hostname !== "github.com" && url.hostname !== "www.github.com") ||
        url.port ||
        url.username ||
        url.password
      )
        return null;
      match = /^\/([a-z\d-]{1,39})\/([a-z\d_.-]{1,100})\/pull\/([1-9]\d*)(?:\/|$)/i.exec(url.pathname);
    } catch {
      return null;
    }
  }
  if (!match || match[2] === "." || match[2] === ".." || Number(match[3]) > 0xffffffff) return null;
  return { repo: { owner: match[1]!, name: match[2]! }, number: Number(match[3]) };
}
