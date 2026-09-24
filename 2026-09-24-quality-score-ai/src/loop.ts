// Score -> ask Claude to refactor -> re-score -> keep only if the score improved and tests still pass.
import { execFileSync, spawnSync } from "node:child_process";
import { cpSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { collect, report, score, type Score } from "./score.ts";

const { values: args } = parseArgs({
  options: {
    fixture: { type: "string", default: "fixtures/order-service" },
    iterations: { type: "string", default: "3" },
    model: { type: "string", default: "sonnet" },
    judge: { type: "boolean", default: false },
  },
});

const TEST_FILE = "src/order.test.ts";
const runDir = resolve("runs", new Date().toISOString().replace(/[:.]/g, "-"));
const work = join(runDir, "work");
mkdirSync(runDir, { recursive: true });
cpSync(resolve(args.fixture!), work, { recursive: true });

const git = (...a: string[]) => execFileSync("git", a, { cwd: work, encoding: "utf8" });
git("init", "-q");
git("add", "-A");
git("-c", "user.name=loop", "-c", "user.email=loop@localhost", "commit", "-q", "-m", "baseline");

const hash = (p: string) => createHash("sha256").update(readFileSync(join(work, p))).digest("hex");
const testHash = hash(TEST_FILE);

function claude(prompt: string, tools: string): string {
  const res = spawnSync(
    "claude",
    ["-p", prompt, "--model", args.model!, "--allowedTools", tools, "--permission-mode", "acceptEdits"],
    { cwd: work, encoding: "utf8", timeout: 15 * 60 * 1000 },
  );
  return (res.stdout ?? "") + (res.stderr ?? "");
}

function refactorPrompt(rep: string): string {
  return `You are improving code quality in this TypeScript project (src/).
A metrics tool produced the report below. Refactor to reduce cognitive/cyclomatic complexity
and remove duplicated logic, so that the score goes up.

Rules:
- Do NOT modify ${TEST_FILE}. Behavior must stay identical; the tests must still pass.
- Keep every exported name and signature unchanged.
- Prefer readable code. Do not obfuscate or cram logic into one-liners just to game the metric.
- You may run: node --test ${TEST_FILE}

Report:
${rep}`;
}

type Step = { iteration: number; before: Score; after: Score; accepted: boolean; reason: string };
const history: Step[] = [];

let m = collect(work);
let current = score(m);
const baseline = current;
console.log(`baseline: ${current.total}`);
console.log(report(m, current));

for (let i = 1; i <= Number(args.iterations); i++) {
  const log = claude(refactorPrompt(report(m, current)), "Read Edit Write Glob Grep Bash(node --test:*)");
  writeFileSync(join(runDir, `claude-${i}.log`), log);

  const nextM = collect(work);
  const next = score(nextM);
  let reason = "";
  if (hash(TEST_FILE) !== testHash) reason = "test file modified";
  else if (!next.testsPassed) reason = "tests failed";
  else if (next.total <= current.total) reason = "score did not improve";

  const accepted = reason === "";
  history.push({ iteration: i, before: current, after: next, accepted, reason: reason || "improved" });
  console.log(`iteration ${i}: ${current.total} -> ${next.total} ${accepted ? "ACCEPT" : `REJECT (${reason})`}`);

  if (accepted) {
    git("add", "-A");
    git("-c", "user.name=loop", "-c", "user.email=loop@localhost", "commit", "-q", "-m", `iteration ${i}`);
    m = nextM;
    current = next;
  } else {
    git("reset", "-q", "--hard");
    git("clean", "-q", "-fd");
  }
}

let judge: string | undefined;
if (args.judge) {
  const diff = git("diff", "HEAD~" + history.filter((h) => h.accepted).length, "--", "src");
  judge = diff
    ? claude(
        `Review this refactoring diff as a senior engineer. Ignoring any metrics, is the code after the change
more readable and maintainable than before? Answer with a line "VERDICT: better|same|worse" and a short reason.
Point out anything that looks like gaming a complexity/duplication metric.

${diff}`,
        "",
      )
    : "no accepted changes";
  writeFileSync(join(runDir, "judge.md"), judge);
}

const summary = { fixture: args.fixture, model: args.model, baseline, final: current, history, judge };
writeFileSync(join(runDir, "summary.json"), JSON.stringify(summary, null, 2));
writeFileSync(join(runDir, "final-report.md"), report(m, current));
console.log(`final: ${baseline.total} -> ${current.total}`);
console.log(`run dir: ${runDir}`);
