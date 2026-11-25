import type { RouteObject } from "react-router";
import { ShellRoute, StartRoute } from "../screens/shell/ShellRoute";
import { HomeScreen } from "../screens/home/HomeScreen";
import { NotFoundScreen } from "../screens/home/NotFoundScreen";
import { CommentsRoute } from "../screens/comments/CommentsRoute";
import { CarryOversScreen, LogScreen, RoundsScreen, TableScreen } from "../screens/comments/CommentPages";
import { QuestionsScreen } from "../screens/comments/QuestionsScreen";
import { ProgressRedirect } from "../screens/pr/ProgressRedirect";
import { PrScreen } from "../screens/pr/PrScreen";
import { AllCarryOversScreen, WatchingScreen } from "../screens/watching/WatchingScreens";
import { SettingsHelpScreen } from "../screens/settings/SettingsHelpScreen";
import { SETTINGS_PAGES } from "../screens/shell/paths";
import { SettingsRedirect, SettingsScreen } from "../screens/settings/SettingsScreen";
import { TourScreen } from "../screens/tour/TourScreen";
import { FilesScreen } from "../screens/files/FilesScreen";
import { DependenciesScreen } from "../screens/deps/DependenciesScreen";
import { ReviewRoute } from "../screens/review/ReviewRoute";

/**
 * App routes (plan §F.1): My work, the review start page, Settings, watched PRs and all carry-overs share the
 * navigation shell; the PR page and Comments (review-workflow-plan §3) have the review's top bar.
 */
export const appRoutes: RouteObject[] = [
  {
    element: <ShellRoute />,
    children: [
      { path: "/", element: <StartRoute /> },
      { path: "/review", element: <HomeScreen /> },
      { path: "/settings", element: <SettingsRedirect /> },
      ...SETTINGS_PAGES.map(({ id, href }) => ({
        path: href,
        element: <SettingsScreen key={id} page={id} />,
      })),
      { path: "/settings/help", element: <SettingsHelpScreen /> },
      { path: "/watching", element: <WatchingScreen /> },
      { path: "/comments/carry-overs", element: <AllCarryOversScreen /> },
    ],
  },
  { path: "/pr/:owner/:repo/:number", element: <PrScreen /> },
  {
    path: "/pr/:owner/:repo/:number/comments",
    element: <CommentsRoute />,
    children: [
      { index: true, element: <QuestionsScreen /> },
      { path: "table", element: <TableScreen /> },
      { path: "rounds", element: <RoundsScreen /> },
      { path: "carry-overs", element: <CarryOversScreen /> },
      { path: "log", element: <LogScreen /> },
    ],
  },
  {
    path: "/analyses/:analysisId",
    element: <ReviewRoute />,
    children: [
      { index: true, element: <TourScreen /> },
      { path: "files", element: <FilesScreen /> },
      { path: "deps", element: <DependenciesScreen /> },
    ],
  },
  { path: "/analyses/:analysisId/progress", element: <ProgressRedirect /> },
  { path: "*", element: <NotFoundScreen /> },
];
