import type { RouteObject } from "react-router";
import { HomeScreen } from "../screens/home/HomeScreen";
import { NotFoundScreen } from "../screens/home/NotFoundScreen";
import { TourScreen } from "../screens/tour/TourScreen";

/** App routes; add-only (plan §4.3). */
export const appRoutes: RouteObject[] = [
  { path: "/", element: <HomeScreen /> },
  { path: "/analyses/:analysisId", element: <TourScreen /> },
  { path: "*", element: <NotFoundScreen /> },
];
