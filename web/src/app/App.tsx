import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createBrowserRouter } from "react-router";
import { RouterProvider } from "react-router/dom";
import { ApiError } from "../api/client";
import { appRoutes } from "./routes";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // Client errors (4xx) are answers, not outages: retry only server and network failures.
      retry: (count, error) =>
        count < 2 && (!(error instanceof ApiError) || error.status === 0 || error.status >= 500),
      refetchOnWindowFocus: false,
    },
  },
});
const router = createBrowserRouter(appRoutes);

/** The app: query client and router. */
export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  );
}
