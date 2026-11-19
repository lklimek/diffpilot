// Syntax colouring of the review screen: Shiki's core, engine, themes and each grammar load on first use.
import { createShikiHighlight } from "../components/code-view/highlight";

/** The app's `Highlight`, shared by every code view and classic diff. */
export const highlight = createShikiHighlight({
  core: () => import("shiki/core"),
  engine: () => import("shiki/engine/javascript"),
  light: () => import("shiki/themes/github-light-default.mjs"),
  dark: () => import("shiki/themes/github-dark-default.mjs"),
  langs: {
    c: () => import("shiki/langs/c.mjs"),
    cpp: () => import("shiki/langs/cpp.mjs"),
    css: () => import("shiki/langs/css.mjs"),
    docker: () => import("shiki/langs/docker.mjs"),
    go: () => import("shiki/langs/go.mjs"),
    html: () => import("shiki/langs/html.mjs"),
    java: () => import("shiki/langs/java.mjs"),
    javascript: () => import("shiki/langs/javascript.mjs"),
    json: () => import("shiki/langs/json.mjs"),
    jsx: () => import("shiki/langs/jsx.mjs"),
    kotlin: () => import("shiki/langs/kotlin.mjs"),
    markdown: () => import("shiki/langs/markdown.mjs"),
    proto: () => import("shiki/langs/proto.mjs"),
    python: () => import("shiki/langs/python.mjs"),
    rust: () => import("shiki/langs/rust.mjs"),
    shellscript: () => import("shiki/langs/shellscript.mjs"),
    sql: () => import("shiki/langs/sql.mjs"),
    swift: () => import("shiki/langs/swift.mjs"),
    toml: () => import("shiki/langs/toml.mjs"),
    tsx: () => import("shiki/langs/tsx.mjs"),
    typescript: () => import("shiki/langs/typescript.mjs"),
    yaml: () => import("shiki/langs/yaml.mjs"),
  },
});
