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

/** Carry-overs of all pull requests (V5). */
export const ALL_CARRY_OVERS_PATH = "/comments/carry-overs";

/** Watched pull requests (V6). */
export const WATCHING_PATH = "/watching";

/** The PR of the route parameters `owner`, `repo`, `number`. */
export const prOfParams = (params: { owner?: string; repo?: string; number?: string }): PrRef => ({
  repo: { owner: params.owner ?? "", name: params.repo ?? "" },
  number: Number(params.number ?? 0),
});
