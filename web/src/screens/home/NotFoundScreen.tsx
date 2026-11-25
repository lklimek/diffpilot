import { StatusMessage } from "../../components/status/StatusMessage";
import { TopBar } from "../../components/top-bar/TopBar";

/** Unknown route. */
export function NotFoundScreen() {
  return (
    <div className="flex min-h-screen flex-col">
      <TopBar />
      <main id="main" className="grid content-start gap-3 px-8 py-5">
        <StatusMessage kind="error">This page does not exist.</StatusMessage>
        <a href="/">My work</a>
      </main>
    </div>
  );
}
