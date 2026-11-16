import { useEffect, useId, useState } from "react";
import { useNavigate } from "react-router";
import { ApiError } from "../../api/client";
import { useMe, useOpenPr } from "../../api/queries";
import { StatusMessage } from "../../components/status/StatusMessage";
import { TopBar } from "../../components/top-bar/TopBar";

/** Minimal entry (WP3-SCREENS builds the real Home): PR URL → finished analysis → review. */
export function HomeScreen() {
  const me = useMe();
  const openPr = useOpenPr();
  const navigate = useNavigate();
  const inputId = useId();
  const [url, setUrl] = useState("");
  const user = me.data?.user ?? null;

  const opened = openPr.data;
  useEffect(() => {
    if (opened?.state.state === "finished") void navigate(`/analyses/${opened.id}`);
  }, [opened, navigate]);

  return (
    <div className="flex min-h-screen flex-col">
      <TopBar login={user?.login ?? null} />
      <main id="main" className="grid content-start gap-4 px-8 py-5">
        <h1 className="text-[22px] font-bold">Review a pull request</h1>
        {me.isPending && <StatusMessage kind="loading">Loading…</StatusMessage>}
        {me.isError && (
          <StatusMessage kind="error">
            {me.error instanceof ApiError ? me.error.message : "Cannot reach the server."}
          </StatusMessage>
        )}
        {me.isSuccess && !user && (
          <p>
            <a href="/auth/github/login">Sign in with GitHub</a>
          </p>
        )}
        {user && (
          <form
            className="flex max-w-[1400px] flex-wrap items-end gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              openPr.mutate(url.trim());
            }}
          >
            <div className="min-w-0 flex-1">
              <label htmlFor={inputId} className="mb-1 block text-xs font-semibold">
                GitHub pull request URL
              </label>
              <input
                id={inputId}
                type="url"
                required
                value={url}
                onChange={(event) => setUrl(event.target.value)}
                placeholder="https://github.com/owner/repo/pull/123"
                className="w-full rounded-md border border-muted bg-surface px-2.5 py-1.5"
              />
            </div>
            <button
              type="submit"
              disabled={openPr.isPending}
              className="cursor-pointer rounded-md border border-accent bg-accent px-3 py-1.5 font-semibold text-on-accent disabled:opacity-55"
            >
              Analyze
            </button>
          </form>
        )}
        {openPr.isError && (
          <StatusMessage kind="error">
            {openPr.error instanceof ApiError ? openPr.error.message : "Something went wrong. Try again."}
          </StatusMessage>
        )}
        {opened && opened.state.state !== "finished" && (
          <StatusMessage kind="empty">
            The analysis has started. This build has no progress screen yet; open the review when it has
            finished.
          </StatusMessage>
        )}
      </main>
    </div>
  );
}
