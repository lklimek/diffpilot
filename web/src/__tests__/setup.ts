import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, beforeEach } from "vitest";
import { resetKeyboardModality } from "../components/popover/Popover";

// Each test starts as a page load: no pointer input yet.
beforeEach(() => resetKeyboardModality());
afterEach(() => cleanup());
