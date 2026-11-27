import { scenario } from "../scenarios";
import { HttpResponse, type HttpResponseResolver, type PathParams } from "msw";
import type {
  AnchoredAskRequest,
  ChannelKind,
  DecideRequest,
  DecisionBatchRequest,
  EffectsPreviewRequest,
  GateMarkRequest,
  ItemActionRequest,
  NotificationChannel,
  PutChannelRequest,
  UserStreamEvent,
} from "../../api/generated/types";
import {
  PR,
  carryOvers,
  context,
  decide,
  decideBatch,
  decideRestList,
  itemOfPath,
  listItems,
  log,
  markGate,
  markedItems,
  notificationSettings,
  overview,
  prPage,
  previewEffects,
  reopen,
  setChannels,
  setWatched,
  skip,
  undo,
  watches,
  type Refusal,
} from "../review-loop";
import { readStore } from "../review-store";
import { recordPrOpened } from "./pr-history";
import { answerLater } from "./ask-question";
import { apiError, route } from "./common";

/** The reader's questions about comment items (RC7). */
const itemQuestions = () => readStore("questions").filter((q) => q.anchor.kind === "comment_item");

/** Items with the answers of the Ask chat about each counted in. */
const itemsWithAsks = () => {
  const answered = itemQuestions().filter((q) => q.state === "answered");
  return listItems().map((i) => ({
    ...i,
    ask_answers:
      i.ask_answers +
      answered.filter((q) => q.anchor.kind === "comment_item" && q.anchor.item === i.id).length,
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
const isRefusal = (value: unknown): value is Refusal => Array.isArray(value) && typeof value[0] === "number";

/** RC1–RC17, RW1–RW2, RN1–RN3 of the review loop (plan §6) over the store of review-loop.ts. */
export const reviewLoopHandlers = [
  route("listMarkedItems", ({ request }) => {
    const result = markedItems(new URL(request.url).searchParams.get("cursor"));
    return isRefusal(result) ? refused(result) : HttpResponse.json(result);
  }),
  route("getPrPage", (info) => {
    if (info.params.owner === "acme" && info.params.name === "private-repo") {
      return apiError(
        422,
        "private_repository",
        "This repository is private. Only public repositories can be analysed.",
      );
    }
    return forPr(() => {
      const page = prPage();
      recordPrOpened(page.pr, page.title);
      return HttpResponse.json(page);
    })(info);
  }),
  route(
    "getComments",
    forPr(() => HttpResponse.json(overview())),
  ),
  route(
    "visitComments",
    forPr(() => new HttpResponse(null, { status: 204 })),
  ),
  route(
    "readComments",
    forPr(() => new HttpResponse(null, { status: 202 })),
  ),
  route(
    "startCommentAssessment",
    forPr(() => new HttpResponse(null, { status: 202 })),
  ),
  route(
    "listCommentItems",
    forPr(({ request }) => {
      const all = itemsWithAsks();
      const unseen = new URL(request.url).searchParams.get("imported_unseen") === "true";
      return HttpResponse.json(unseen ? all.filter((item) => item.imported_decision) : all);
    }),
  ),
  route(
    "getCommentContext",
    forPr(({ params, request }) => {
      const no = itemOfPath(String(params.item))?.no;
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
      return isRefusal(result) ? refused(result) : HttpResponse.json(result, { status: 201 });
    }),
  ),
  route(
    "decideCommentBatch",
    forPr(async ({ request }) => {
      const body = (await request.json()) as DecisionBatchRequest;
      const result = decideBatch(body.via, body.rows);
      return isRefusal(result) ? refused(result) : HttpResponse.json(result, { status: 201 });
    }),
  ),
  route(
    "previewCommentEffects",
    forPr(async ({ request }) => {
      const result = previewEffects(((await request.json()) as EffectsPreviewRequest).rows);
      return isRefusal(result) ? refused(result) : HttpResponse.json(result);
    }),
  ),
  route(
    "getDecideRest",
    forPr(() => HttpResponse.json(decideRestList())),
  ),
  route(
    "undoCommentDecision",
    forPr(async ({ params, request }) => {
      const body = (await request.json()) as ItemActionRequest;
      const result = undo(String(params.item), body.item_revision);
      return isRefusal(result) ? refused(result) : HttpResponse.json(result);
    }),
  ),
  route(
    "reopenCommentItem",
    forPr(async ({ params, request }) => {
      const body = (await request.json()) as ItemActionRequest;
      const result = reopen(String(params.item), body.item_revision);
      return isRefusal(result) ? refused(result) : HttpResponse.json(result);
    }),
  ),
  route(
    "skipCommentItem",
    forPr(async ({ params, request }) => {
      const body = (await request.json()) as ItemActionRequest;
      const result = skip(String(params.item), body.item_revision);
      return result ? refused(result) : new HttpResponse(null, { status: 204 });
    }),
  ),
  route(
    "markCommentGate",
    forPr(async ({ params, request }) => {
      const result = markGate(String(params.item), (await request.json()) as GateMarkRequest);
      return isRefusal(result) ? refused(result) : HttpResponse.json(result);
    }),
  ),
  // RC7: the reader's review history; standalone comments have no analysis event stream.
  route(
    "listPrQuestions",
    forPr(() => HttpResponse.json(overview().analysis ? readStore("questions") : itemQuestions())),
  ),
  route(
    "askPrQuestion",
    forPr(async ({ request }) => {
      const { text, anchor, parent_id } = (await request.json()) as AnchoredAskRequest;
      if (anchor.kind !== "comment_item")
        return apiError(400, "invalid_request", "Ask here about one review comment of the pull request.");
      if (!listItems().some((i) => i.id === anchor.item))
        return apiError(404, "not_found", "The comment is not part of this pull request. Reload the page.");
      const questions = itemQuestions();
      const parent = parent_id ? questions.find((q) => q.id === parent_id) : undefined;
      const root = parent?.parent_id ? questions.find((q) => q.id === parent.parent_id) : parent;
      if (parent_id && !root)
        return apiError(404, "not_found", "The question to follow up was not found. Reload the review.");
      return HttpResponse.json(answerLater(anchor, text, root?.id), { status: 202 });
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
  // RW3: keep the stream open as it is for a signed-in user.
  route(
    "userEvents",
    () =>
      new HttpResponse(
        new ReadableStream({
          start(controller) {
            if (!scenario().reviewLoopNotification) return;
            const event = {
              event: "user_event",
              user_event: {
                kind: "questions_ready",
                id: "0f7c1d1e-0000-4000-8000-000000000001",
                pr: PR,
                text: "acme/node-client #482: 3 questions ready",
                count: 3,
                url: "/pr/acme/node-client/482/comments",
                at: "2026-11-18T09:27:00Z",
              },
            } satisfies UserStreamEvent;
            controller.enqueue(
              new TextEncoder().encode(`event: user_event\ndata: ${JSON.stringify(event)}\n\n`),
            );
          },
        }),
        {
          headers: { "Content-Type": "text/event-stream", "Cache-Control": "no-store" },
        },
      ),
  ),
  route("getNotificationSettings", () => HttpResponse.json(notificationSettings())),
  route("putNotificationChannel", async ({ params, request }) => {
    const kind = params.kind as ChannelKind;
    const body = (await request.json()) as PutChannelRequest;
    if (!/^https?:\/\//.test(body.url))
      return apiError(422, "scheme_not_allowed", "Only http and https addresses are allowed.");
    const current = notificationSettings();
    const old = current.channels.find((c) => c.kind === kind);
    // notify.rs `check_target`: a kept secret never follows a changed URL origin (scheme, host, port).
    const origin = (url: string) => {
      try {
        return new URL(url).origin;
      } catch {
        return null;
      }
    };
    const kept = (body.secret?.action ?? "keep") === "keep";
    if (old?.secret_last4 && kept && (origin(old.url) === null || origin(old.url) !== origin(body.url)))
      return apiError(
        422,
        "secret_required",
        "The kind or the URL's scheme, host or port changed: replace or remove the secret, it is not sent to a new destination.",
      );
    const saved: NotificationChannel = {
      kind,
      url: body.url,
      topic: body.topic,
      secret_last4:
        body.secret?.action === "replace"
          ? body.secret.value.slice(-4)
          : body.secret?.action === "remove"
            ? null
            : (old?.secret_last4 ?? null),
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
