// The Files view's file list of the fixture PR (`files-482.json`, projected from the tour seed, the lines seed and
// repo/hunks.json like the server's R12), with the Viewed marks the user set and the owners of a lazy tour.
import type { FilesResponse, HunkPart, Tour } from "../api/generated/types";
import seed from "./files-482.json";
import { readStore } from "./review-store";
import { scenario } from "./scenarios";

/** `GET …/files` for `tour` (the tour the mocks serve now): steps it does not hold yet are their flow's plan. */
export function mockFiles(tour: Tour): FilesResponse {
  const steps = new Set(tour.flows.flatMap((f) => f.steps.map((s) => s.id)));
  const viewed = readStore("viewed");
  const owner = (part: HunkPart): HunkPart => ({
    ...part,
    owner:
      part.owner.kind === "step" && !steps.has(part.owner.step_id)
        ? { kind: "flow", flow_id: part.owner.flow_id }
        : part.owner,
    also_in: part.also_in.filter((id) => steps.has(id)),
  });
  const files = structuredClone(seed as FilesResponse);
  const answer: FilesResponse = {
    ...files,
    files: files.files.map((file) => ({
      ...file,
      viewed: viewed[file.path] ?? file.viewed,
      hunks: file.hunks.map((hunk) => ({ ...hunk, parts: hunk.parts.map(owner) })),
    })),
  };
  return scenario().files?.(answer) ?? answer;
}
