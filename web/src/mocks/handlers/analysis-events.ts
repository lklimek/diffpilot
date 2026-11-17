import { HttpResponse } from "msw";
import type { AnalysisEvent, AskQuestion } from "../../api/generated/types";
import { seedNow } from "../fixtures";
import { liveAnalysis, subscribeLive } from "../live-analysis";
import { forAnalysis, route } from "./common";

/** `GET /api/v1/analyses/{id}/events` (sse-events.md): snapshot (analysis, every question), then live events. */
export const analysisEvents = route(
  "analysisEvents",
  forAnalysis(({ request }) => {
    const encoder = new TextEncoder();
    let unsubscribe = () => {};
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        const send = (event: AnalysisEvent) => {
          try {
            controller.enqueue(encoder.encode(`event: ${event.event}\ndata: ${JSON.stringify(event)}\n\n`));
          } catch {
            unsubscribe();
          }
        };
        unsubscribe = subscribeLive((summary) => send({ event: "analysis", summary }));
        send({ event: "analysis", summary: liveAnalysis() });
        for (const question of seedNow("questions") as AskQuestion[]) send({ event: "question", question });
        request.signal.addEventListener("abort", () => unsubscribe());
      },
      cancel() {
        unsubscribe();
      },
    });
    return new HttpResponse(body, {
      headers: { "Content-Type": "text/event-stream", "Cache-Control": "no-store" },
    });
  }),
);
