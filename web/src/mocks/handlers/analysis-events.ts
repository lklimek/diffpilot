import { HttpResponse } from "msw";
import type { AnalysisEvent } from "../../api/generated/types";
import { HEARTBEAT_MS, liveAnalysis, openStream, playScriptOnce, subscribeLive } from "../live-analysis";
import { readStore, subscribeQuestions } from "../review-store";
import { scenario } from "../scenarios";
import { forAnalysis, route } from "./common";

/**
 * `GET /api/v1/analyses/{id}/events` (sse-events.md): snapshot (analysis, every question), then live events and a
 * heartbeat every `HEARTBEAT_MS`.
 */
export const analysisEvents = route(
  "analysisEvents",
  forAnalysis(({ request }) => {
    const encoder = new TextEncoder();
    let unsubscribe = () => {};
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        // A dead server behind an open socket: the later streams never say anything.
        if (openStream() > 0 && scenario().silent) return;
        const send = (event: AnalysisEvent) => {
          try {
            controller.enqueue(encoder.encode(`event: ${event.event}\ndata: ${JSON.stringify(event)}\n\n`));
          } catch {
            unsubscribe();
          }
        };
        const unsubscribeLive = subscribeLive((summary) => send({ event: "analysis", summary }));
        const unsubscribeQuestions = subscribeQuestions((question) => send({ event: "question", question }));
        const heartbeat = scenario().silent
          ? undefined
          : setInterval(() => send({ event: "heartbeat", at: new Date().toISOString() }), HEARTBEAT_MS);
        unsubscribe = () => {
          clearInterval(heartbeat);
          unsubscribeLive();
          unsubscribeQuestions();
        };
        send({ event: "analysis", summary: liveAnalysis() });
        for (const question of readStore("questions")) send({ event: "question", question });
        const script = scenario().script;
        if (script) playScriptOnce(script);
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
