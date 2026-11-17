import { HttpResponse } from "msw";
import { MEMCAN_SYNC_MS, readStore, updateStore } from "../review-store";
import { scenario } from "../scenarios";
import { forAnalysis, route } from "./common";

/**
 * `GET /api/v1/analyses/{id}/feedback`. A pending MemCan write ends `MEMCAN_SYNC_MS` after the entry was first
 * listed: `written`, or `failed` in the scenario `memcan-failed`.
 */
export const listFeedback = route(
  "listFeedback",
  forAnalysis(() => {
    const now = Date.now();
    updateStore((store) => {
      store.memorySeenAt ??= {};
      for (const entry of store.feedback) {
        if (entry.memory !== "pending") continue;
        const seen = (store.memorySeenAt[entry.id] ??= now);
        if (now - seen >= MEMCAN_SYNC_MS) entry.memory = scenario().memcan ?? "written";
      }
    });
    return HttpResponse.json(readStore("feedback"));
  }),
);
