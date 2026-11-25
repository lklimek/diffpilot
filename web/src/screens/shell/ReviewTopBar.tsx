import type { ComponentProps } from "react";
import { TopBar } from "../../components/top-bar/TopBar";
import { useSessionSignOut } from "./useSessionSignOut";
import { ShellUserMenu } from "./ShellUserMenu";

/** The review top bar with the same account controls as the navigation shell. */
export function ReviewTopBar({
  login,
  onHelp,
  ...props
}: Omit<ComponentProps<typeof TopBar>, "userMenu"> & {
  login: string | null;
  onHelp?: () => void;
}) {
  const signOut = useSessionSignOut(login);
  return (
    <TopBar
      {...props}
      userMenu={login && <ShellUserMenu key={login} login={login} onHelp={onHelp} signOut={signOut} />}
    />
  );
}
