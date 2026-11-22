import { HttpResponse, type HttpResponseResolver, type PathParams } from "msw";
import type {
  ChannelKind,
  DecideRequest,
  DecisionBatchRequest,
  NotificationChannel,
  PutChannelRequest,
} from "../../api/generated/types";
import {
  PR,
  carryOvers,
  context,
  decide,
  decideBatch,
  listItems,
  log,
  notificationSettings,
  overview,
  prPage,
  setChannels,
  setWatched,
  watches,
  type Refusal,
} from "../review-loop";
import { readStore } from "../review-store";
import { apiError, route } from "./common";

/** Items with the answers of the Ask chat about each (subject `comment_item`, pending contract) counted in. */
const itemsWithAsks = () => {
  const answered = readStore("questions").filter((q) => q.state === "answered");
  return listItems().map((i) => ({
    ...i,
    ask_answers:
      i.ask_answers +
      answered.filter((q) => {
        const anchor = q.anchor as { kind: string; item?: string };
        return anchor.kind === "comment_item" && anchor.item === i.id;
      }).length,
  }));
};

type Resolver = HttpResponseResolver<PathParams>;

/** Resolver of a PR route: `not_found` unless it is the fixture PR. */
const forPr =
  (answer: Resolver): Resolver =>
  (info) => {
    const { owner, name, number } = info.params;
    if (owner !== PR.repo.owner || name !== PR.repo.name || number !== String(PR.number))
      return apiError(404, "not_found", "This pull request is not known to diffpilot.");
    return answer(info);
  };

const refused = (r: Refusal) => apiError(r[0], r[1], r[2]);

/** RC1–RC11, RW1–RW2, RN1–RN3 of the review loop (plan §6) over the store of review-loop.ts. */
export const reviewLoopHandlers = [
  route(
    "getPrPage",
    forPr(() => HttpResponse.json(prPage())),
  ),
  route(
    "getComments",
    forPr(() => HttpResponse.json(overview())),
  ),
  route(
    "readComments",
    forPr(() => new HttpResponse(null, { status: 202 })),
  ),
  route(
    "listCommentItems",
    forPr(() => HttpResponse.json(itemsWithAsks())),
  ),
  route(
    "getCommentContext",
    forPr(({ params, request }) => {
      const no = listItems().find((i) => i.id === params.item)?.no;
      const query = new URL(request.url).searchParams;
      const num = (key: string) => (query.has(key) ? Number(query.get(key)) : null);
      const found = no === undefined ? null : context(no, num("from"), num("to"));
      return found ? HttpResponse.json(found) : apiError(404, "not_found", "This comment has no code lines.");
    }),
  ),
  route(
    "assessComment",
    forPr(() => new HttpResponse(null, { status: 202 })),
  ),
  route(
    "decideComment",
    forPr(async ({ params, request }) => {
      const result = decide(String(params.item), (await request.json()) as DecideRequest);
      return Array.isArray(result) ? refused(result) : HttpResponse.json(result, { status: 201 });
    }),
  ),
  route(
    "decideCommentBatch",
    forPr(async ({ request }) => {
      const body = (await request.json()) as DecisionBatchRequest;
      const result = decideBatch(body.via, body.rows);
      return typeof result[0] === "number"
        ? refused(result as Refusal)
        : HttpResponse.json(result, { status: 201 });
    }),
  ),
  route(
    "getCommentLog",
    forPr(() => HttpResponse.json(log())),
  ),
  route("listCarryOvers", ({ request }) =>
    HttpResponse.json(carryOvers(new URL(request.url).searchParams.get("pr"))),
  ),
  route(
    "watchPr",
    forPr(() => {
      setWatched(true);
      return new HttpResponse(null, { status: 204 });
    }),
  ),
  route(
    "unwatchPr",
    forPr(() => {
      setWatched(false);
      return new HttpResponse(null, { status: 204 });
    }),
  ),
  route("listWatches", () => HttpResponse.json(watches())),
  // RW3: the stream stays open; the mocks create no user events.
  route(
    "userEvents",
    () =>
      new HttpResponse(new ReadableStream({ start() {} }), {
        headers: { "Content-Type": "text/event-stream", "Cache-Control": "no-store" },
      }),
  ),
  route("getNotificationSettings", () => HttpResponse.json(notificationSettings())),
  route("putNotificationChannel", async ({ params, request }) => {
    const kind = params.kind as ChannelKind;
    const body = (await request.json()) as PutChannelRequest;
    if (!/^https?:\/\//.test(body.url))
      return apiError(422, "scheme_not_allowed", "Only http and https addresses are allowed.");
    const current = notificationSettings();
    const old = current.channels.find((c) => c.kind === kind);
    const saved: NotificationChannel = {
      kind,
      url: body.url,
      topic: body.topic,
      secret_last4: body.secret ? body.secret.slice(-4) : (old?.secret_last4 ?? null),
      events: body.events,
      enabled: body.enabled,
      delivery: old?.delivery ?? { last: null, last_error: null, failing_since: null },
    };
    setChannels({ channels: [...current.channels.filter((c) => c.kind !== kind), saved] });
    return HttpResponse.json(saved);
  }),
  route("deleteNotificationChannel", ({ params }) => {
    setChannels({ channels: notificationSettings().channels.filter((c) => c.kind !== params.kind) });
    return new HttpResponse(null, { status: 204 });
  }),
  route("testNotificationChannel", ({ params }) => {
    const ok = params.kind === "ntfy";
    const at = new Date().toISOString();
    setChannels({
      channels: notificationSettings().channels.map((c) =>
        c.kind === params.kind
          ? {
              ...c,
              delivery: ok
                ? { last: { event: "test", outcome: "delivered", at }, last_error: null, failing_since: null }
                : {
                    last: { event: "test", outcome: "failed", at },
                    last_error: "connection refused (ha.example.lan:8123)",
                    failing_since: c.delivery.failing_since ?? at,
                  },
            }
          : c,
      ),
    });
    return HttpResponse.json(
      ok
        ? { outcome: "delivered", http_status: 200, error: null }
        : { outcome: "failed", http_status: null, error: "connection refused (ha.example.lan:8123)" },
    );
  }),
];
