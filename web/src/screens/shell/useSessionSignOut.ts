import { useQueryClient } from "@tanstack/react-query";
import { useRef } from "react";
import { useNavigate } from "react-router";
import { ApiError } from "../../api/client";
import { useSignOut } from "../../api/queries";
import { clearCommentDrafts } from "../tour/drafts";
import { MY_WORK_PATH } from "./paths";

/** The shared logout action for the user menu and Settings. */
export function useSessionSignOut(login: string | null) {
  const mutation = useSignOut();
  const client = useQueryClient();
  const navigate = useNavigate();
  const leaving = useRef(false);

  function leave(everywhere: boolean) {
    if (!login || mutation.isPending) return;
    const signedOut = () => {
      leaving.current = true;
      clearCommentDrafts(login);
      client.clear();
      void navigate(MY_WORK_PATH, { replace: true });
    };
    mutation.mutate(everywhere, {
      onSuccess: signedOut,
      onError: (error) => {
        if (!everywhere && error instanceof ApiError && error.status === 401) signedOut();
      },
    });
  }

  const error = mutation.error;
  const everywhere = mutation.variables === true;
  const errorText =
    everywhere && error instanceof ApiError && error.status === 401
      ? "your session had already ended, so nothing was revoked. Other sessions and personal access tokens still work; sign in and choose Sign out everywhere again."
      : error instanceof ApiError
        ? error.message
        : error
          ? "Something went wrong. Reload the page."
          : undefined;

  return { leave, leaving, pending: mutation.isPending, error: errorText, everywhere };
}
