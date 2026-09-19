// The vocabulary check (`docs/plan/07-ui-vocabulary.md` §2, M5-T02).
//
// Engine words are allowed in exactly one place in the frontend:
// `src/vocab/`. Everywhere a person can read them — the screens and the
// components — a string that says "commit", "repository" or "stderr" is a
// word the Everyday mode was built to keep off the screen, and one that got
// past `t()`.
//
// The check parses. A grep would fail on `JSON.stringify` and on every
// variable called `token`, and failing on those teaches people to silence
// the check rather than fix the copy. So this walks the TypeScript AST and
// looks at three node kinds only: string literals, template literals, and
// JSX text. Identifiers, property names, comments and types are code, and
// code may say what it likes.
//
// Run: `node scripts/check-vocab.mjs` (or `pnpm check-vocab` in apps/desktop).

import { readFileSync, readdirSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const app = path.join(root, "apps", "desktop");

// `typescript` lives in the desktop app's dependencies, not the repo root's,
// and a bare import here would look beside this file instead.
const require = createRequire(path.join(app, "package.json"));
let ts;
try {
  ts = require("typescript");
} catch {
  console.error(
    "check-vocab: typescript is not installed. Run `pnpm install` in apps/desktop first.",
  );
  process.exit(2);
}

/** The words the screens must not say (§2). Matched case-insensitively. */
const BANNED = [
  "commit",
  "repo",
  "repository",
  "diff",
  "MCP",
  "skill",
  "stdout",
  "stderr",
  "stack trace",
  "JSON",
  "token",
];

// Plurals and the usual verb endings, so "commits" and "committing" are
// caught along with "commit". The boundaries keep "different" and
// "difference" out of it: those are ordinary English, not engine words.
const patterns = BANNED.map((word) => ({
  word,
  re: new RegExp(`\\b${word.replace(/ /g, "\\s+")}(s|es|ed|ing|ted|ting|ded|ding)?\\b`, "i"),
}));

/** The directories a person reads text out of. `vocab/` is where it may live. */
const SCANNED = [
  path.join(app, "src", "screens"),
  path.join(app, "src", "components"),
  // The shell renders navigation text of its own, so it plays by the same
  // rule even though it is not in either folder.
  path.join(app, "src", "App.tsx"),
];

function sources(target) {
  let stat;
  try {
    stat = statSync(target);
  } catch {
    return [];
  }
  if (stat.isFile()) return /\.tsx?$/.test(target) ? [target] : [];
  return readdirSync(target).flatMap((entry) => sources(path.join(target, entry)));
}

/**
 * Whether this node is code that merely happens to be a string: an
 * `import ... from "./x"` path, or a `className`, which names a CSS rule and
 * never reaches the screen as words.
 */
function isCodeNotCopy(node) {
  const parent = node.parent;
  if (!parent) return false;
  if (
    (ts.isImportDeclaration(parent) || ts.isExportDeclaration(parent)) &&
    parent.moduleSpecifier === node
  ) {
    return true;
  }
  if (ts.isImportTypeNode(parent) || ts.isExternalModuleReference(parent)) return true;
  if (ts.isJsxAttribute(parent) && parent.name.getText() === "className") return true;
  if (
    ts.isJsxExpression(parent) &&
    parent.parent &&
    ts.isJsxAttribute(parent.parent) &&
    parent.parent.name.getText() === "className"
  ) {
    return true;
  }
  return false;
}

const findings = [];

for (const file of SCANNED.flatMap(sources)) {
  const text = readFileSync(file, "utf8");
  const source = ts.createSourceFile(file, text, ts.ScriptTarget.ES2022, true, ts.ScriptKind.TSX);

  const look = (node, value) => {
    if (isCodeNotCopy(node)) return;
    for (const { word, re } of patterns) {
      const hit = re.exec(value);
      if (!hit) continue;
      const { line, character } = source.getLineAndCharacterOfPosition(node.getStart(source));
      findings.push({
        file: path.relative(root, file),
        line: line + 1,
        column: character + 1,
        word,
        found: hit[0],
        text: value.trim().replace(/\s+/g, " ").slice(0, 80),
      });
      return; // One finding per string is enough to send someone to the line.
    }
  };

  const walk = (node) => {
    if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) {
      look(node, node.text);
    } else if (ts.isTemplateExpression(node)) {
      // Only the literal parts: the holes are expressions, walked below.
      look(node, node.head.text + node.templateSpans.map((span) => span.literal.text).join(" "));
    } else if (ts.isJsxText(node)) {
      if (node.text.trim()) look(node, node.text);
    }
    ts.forEachChild(node, walk);
  };
  walk(source);
}

// ---- the second rule: Everyday mode says none of it either ---------------
//
// The walk above proves the engine words live in `vocab/`. It does not prove
// they are kept out of the Everyday renderings, and that is the rule M5-T06
// is actually about: a person who never chose Developer mode must not be
// shown the word "commit" merely because it was spelled inside the
// dictionary. So the `everyday:` side of every entry is held to the same
// list. The `developer:` side is exempt — naming the machinery is what it is
// for.

const dictionaryFile = path.join(app, "src", "vocab", "dictionary.ts");
const dictionarySource = ts.createSourceFile(
  dictionaryFile,
  readFileSync(dictionaryFile, "utf8"),
  ts.ScriptTarget.ES2022,
  true,
  ts.ScriptKind.TS,
);

/** The text of an `everyday:` value, when it is a plain string. */
function everydayText(node) {
  if (!ts.isPropertyAssignment(node)) return null;
  if (node.name.getText(dictionarySource) !== "everyday") return null;
  const value = node.initializer;
  if (ts.isStringLiteral(value) || ts.isNoSubstitutionTemplateLiteral(value)) return value.text;
  return null; // `null` means hidden, and anything else is not copy.
}

/**
 * Which dictionary key this sits under: the nearest enclosing property whose
 * value is the entry itself — an object literal for `{ everyday, developer }`
 * entries, a `same(...)` call for the ones that read the same in both modes.
 */
function keyOf(node) {
  for (let up = node.parent; up; up = up.parent) {
    if (
      ts.isPropertyAssignment(up) &&
      (ts.isObjectLiteralExpression(up.initializer) || ts.isCallExpression(up.initializer))
    ) {
      return up.name.getText(dictionarySource);
    }
  }
  return "(unknown)";
}

const everydayWalk = (node) => {
  const text = everydayText(node);
  if (text !== null) {
    for (const { word, re } of patterns) {
      const hit = re.exec(text);
      if (!hit) continue;
      const { line } = dictionarySource.getLineAndCharacterOfPosition(node.getStart(dictionarySource));
      findings.push({
        file: path.relative(root, dictionaryFile),
        line: line + 1,
        column: 1,
        word,
        found: hit[0],
        text: `${keyOf(node)}: "${text.slice(0, 60)}"`,
        everyday: true,
      });
      break;
    }
  }
  ts.forEachChild(node, everydayWalk);
};
everydayWalk(dictionarySource);

// `same(...)` puts one string in both modes, so its text is Everyday copy too.
const sameWalk = (node) => {
  if (
    ts.isCallExpression(node) &&
    node.expression.getText(dictionarySource) === "same" &&
    node.arguments.length === 1
  ) {
    const argument = node.arguments[0];
    if (ts.isStringLiteral(argument) || ts.isNoSubstitutionTemplateLiteral(argument)) {
      for (const { word, re } of patterns) {
        const hit = re.exec(argument.text);
        if (!hit) continue;
        const { line } = dictionarySource.getLineAndCharacterOfPosition(node.getStart(dictionarySource));
        findings.push({
          file: path.relative(root, dictionaryFile),
          line: line + 1,
          column: 1,
          word,
          found: hit[0],
          text: `${keyOf(node)}: same("${argument.text.slice(0, 60)}")`,
          everyday: true,
        });
        break;
      }
    }
  }
  ts.forEachChild(node, sameWalk);
};
sameWalk(dictionarySource);

if (findings.length === 0) {
  console.log(
    "check-vocab: every word on the screens comes from the dictionary, " +
      "and Everyday mode speaks none of the engine's.",
  );
  process.exit(0);
}

const onScreens = findings.filter((finding) => !finding.everyday).length;
const inEveryday = findings.length - onScreens;
const parts = [];
if (onScreens > 0) {
  parts.push(
    `${onScreens} string${onScreens === 1 ? "" : "s"} outside vocab/ ` +
      `say${onScreens === 1 ? "s" : ""} what only the dictionary may say`,
  );
}
if (inEveryday > 0) {
  parts.push(
    `${inEveryday} Everyday rendering${inEveryday === 1 ? "" : "s"} ` +
      `speak${inEveryday === 1 ? "s" : ""} the engine's vocabulary`,
  );
}
console.error(`check-vocab: ${parts.join(", and ")}:\n`);
for (const finding of findings) {
  console.error(`  ${finding.file}:${finding.line}:${finding.column}`);
  console.error(`    ${finding.everyday ? finding.text : `"${finding.text}"`}`);
  console.error(
    finding.everyday
      ? `    says "${finding.found}" in Everyday mode — say it in plain words, and keep the engine's word for the developer: side.\n`
      : `    says "${finding.found}" — put the wording in src/vocab/dictionary.ts and call t().\n`,
  );
}
process.exit(1);
