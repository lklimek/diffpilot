import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useMemo, useRef } from "react";
import { useNavigate } from "react-router";
import { ApiError } from "../../api/client";
import { useSignOut } from "../../api/queries";
import { clearCommentDrafts } from "../tour/drafts";
import { MY_WORK_PATH } from "./paths";

/** The shared logout action for the user menu and Settings. */
export function useSessionSignOut(login: string | null) {
  const { mutate, isPending, error, variables } = useSignOut();
  const client = useQueryClient();
  const navigate = useNavigate();
  const leaving = useRef(false);

  const leave = useCallback(
    (everywhere: boolean) => {
      if (!login || isPending) return;
      const signedOut = () => {
        leaving.current = true;
        clearCommentDrafts(login);
        client.clear();
        void navigate(MY_WORK_PATH, { replace: true });
      };
      mutate(everywhere, {
        onSuccess: signedOut,
        onError: (error) => {
          if (!everywhere && error instanceof ApiError && error.status === 401) signedOut();
        },
      });
    },
    [client, isPending, login, mutate, navigate],
  );

  const everywhere = variables === true;
  const errorText =
    everywhere && error instanceof ApiError && error.status === 401
      ? "your session had already ended, so nothing was revoked. Other sessions and personal access tokens still work; sign in and choose Sign out everywhere again."
      : error instanceof ApiError
        ? error.message
        : error
          ? "Something went wrong. Reload the page."
          : undefined;

  return useMemo(
    () => ({ leave, leaving, pending: isPending, error: errorText, everywhere }),
    [leave, isPending, errorText, everywhere],
  );
}
