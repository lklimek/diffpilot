// `commenting_available: false` is one boolean for "App not installed", "unusable as configured" and "check failed".

/** What the UI can say when commenting is not available (the cause is not known to the browser). */
export const COMMENTING_UNAVAILABLE = "Commenting is not available for this repository right now.";

/** The possible causes, phrased as possibilities. */
export const COMMENTING_UNAVAILABLE_WHY =
  "The smart-review GitHub App may not be installed on it, or its installation could not be checked.";
