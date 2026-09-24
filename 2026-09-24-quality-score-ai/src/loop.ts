// Score -> ask Claude to refactor -> re-score -> keep only if the score improved and tests still pass.
import { execFileSync, spawn, spawnSync } from "node:child_process";
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
    // score: accept only when the score strictly improves (run1).
    // judge: accept when the score does not drop and a reviewer says the diff is "better".
    accept: { type: "string", default: "score" },
    // Number of independent judges per iteration (run in parallel); accept on a strict majority of "better".
    judges: { type: "string", default: "1" },
    // Pass the reasons for rejected attempts (incl. judge reviews) into the next refactor prompt.
    feedback: { type: "boolean", default: false },
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

function claudeAsync(prompt: string): Promise<string> {
  return new Promise((done) => {
    const p = spawn("claude", ["-p", prompt, "--model", args.model!, "--allowedTools", ""], {
      cwd: work,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let out = "";
    p.stdout.on("data", (c) => (out += c));
    p.stderr.on("data", (c) => (out += c));
    p.on("close", () => done(out));
  });
}

function refactorPrompt(rep: string, feedback: string[]): string {
  const fb = feedback.length
    ? `

Previous attempts were REJECTED and rolled back. Learn from the reviewers' feedback below.
If a reported function is already readable and further change would only add indirection,
leave it as it is: a lower metric score is acceptable, a less readable change is not.
Making no change at all is an acceptable outcome.

${feedback.map((f, k) => `### Rejected attempt ${k + 1}\n${f}`).join("\n\n")}`
    : "";
  return `You are improving code quality in this TypeScript project (src/).
A metrics tool produced the report below. Refactor to reduce cognitive/cyclomatic complexity
and remove duplicated logic, so that the score goes up.

Rules:
- Do NOT modify ${TEST_FILE}. Behavior must stay identical; the tests must still pass.
- Keep every exported name and signature unchanged.
- Prefer readable code. Do not obfuscate or cram logic into one-liners just to game the metric.
- You may run: node --test ${TEST_FILE}

Report:
${rep}${fb}`;
}

type Step = {
  iteration: number;
  before: Score;
  after: Score;
  accepted: boolean;
  reason: string;
  verdict?: string;
  votes?: string[];
  cognitive: { sum: number; max: number; functions: number };
};

const judgePrompt = (diff: string) => `Review this refactoring diff as a senior engineer. Ignoring any metrics, is the code after the change
more readable and maintainable than before? Answer with a line "VERDICT: better|same|worse" and a short reason.
Point out anything that looks like gaming a complexity/duplication metric.

${diff}`;

const parseVerdict = (text: string) => text.match(/VERDICT:\W*(better|same|worse)/i)?.[1]?.toLowerCase() ?? "unknown";

async function judgeVotes(diff: string, n: number): Promise<{ verdict: string; votes: string[]; texts: string[] }> {
  const texts = await Promise.all(Array.from({ length: n }, () => claudeAsync(judgePrompt(diff))));
  const votes = texts.map(parseVerdict);
  const better = votes.filter((v) => v === "better").length;
  return { verdict: better * 2 > n ? "better" : "not-better", votes, texts };
}
const history: Step[] = [];
const feedback: string[] = [];
const MAX_FEEDBACK = 2;
const clean = (t: string) => t.replace(/^Warning: no stdin.*\n/m, "").trim().slice(0, 2000);

let m = collect(work);
const m0 = m;
let current = score(m);
const baseline = current;
console.log(`baseline: ${current.total}`);
console.log(report(m, current));

for (let i = 1; i <= Number(args.iterations); i++) {
  const prompt = refactorPrompt(report(m, current), args.feedback ? feedback : []);
  writeFileSync(join(runDir, `prompt-${i}.md`), prompt);
  const log = claude(prompt, "Read Edit Write Glob Grep Bash(node --test:*)");
  writeFileSync(join(runDir, `claude-${i}.log`), log);

  const nextM = collect(work);
  const next = score(nextM);
  let reason = "";
  let verdict: string | undefined;
  let votes: string[] | undefined;
  let reviews: string[] = [];
  if (hash(TEST_FILE) !== testHash) reason = "test file modified";
  else if (!next.testsPassed) reason = "tests failed";
  else if (args.accept === "score") {
    if (next.total <= current.total) reason = "score did not improve";
  } else {
    const diff = git("diff", "HEAD", "--", "src");
    if (next.total < current.total) reason = "score dropped";
    else if (!diff) reason = "no change";
    else {
      const j = await judgeVotes(diff, Number(args.judges));
      verdict = j.verdict;
      votes = j.votes;
      reviews = j.texts.map(clean);
      j.texts.forEach((t, k) => writeFileSync(join(runDir, `judge-${i}-${k + 1}.md`), t));
      if (verdict !== "better") reason = `judge: ${votes.join("/")}`;
    }
  }

  const accepted = reason === "";
  const cognitive = {
    sum: nextM.functions.reduce((n, f) => n + f.cognitive, 0),
    max: Math.max(...nextM.functions.map((f) => f.cognitive)),
    functions: nextM.functions.length,
  };
  history.push({ iteration: i, before: current, after: next, accepted, reason: reason || "accepted", verdict, votes, cognitive });
  console.log(
    `iteration ${i}: ${current.total} -> ${next.total}${votes ? ` votes=${votes.join("/")}` : ""} ${accepted ? "ACCEPT" : `REJECT (${reason})`}`,
  );

  if (accepted) {
    git("add", "-A");
    git("-c", "user.name=loop", "-c", "user.email=loop@localhost", "commit", "-q", "-m", `iteration ${i}`);
    m = nextM;
    current = next;
  } else {
    if (reason !== "no change") {
      const body = reviews.length
        ? reviews.map((r, k) => `Reviewer ${k + 1}:\n${r}`).join("\n\n")
        : "(no reviewer comments)";
      feedback.push(`Reason: ${reason}\n\n${body}`);
      if (feedback.length > MAX_FEEDBACK) feedback.shift();
    }
    git("reset", "-q", "--hard");
    git("clean", "-q", "-fd");
  }
}

let judge: string | undefined;
if (args.judge) {
  const diff = git("diff", "HEAD~" + history.filter((h) => h.accepted).length, "--", "src");
  judge = diff ? (await judgeVotes(diff, 1)).texts[0] : "no accepted changes";
  writeFileSync(join(runDir, "judge.md"), judge);
}

const baseCognitive = {
  sum: m0.functions.reduce((n, f) => n + f.cognitive, 0),
  max: Math.max(...m0.functions.map((f) => f.cognitive)),
  functions: m0.functions.length,
};
const summary = { fixture: args.fixture, model: args.model, accept: args.accept, judges: Number(args.judges), feedback: args.feedback, baseline, baseCognitive, final: current, history, judge };
writeFileSync(join(runDir, "summary.json"), JSON.stringify(summary, null, 2));
writeFileSync(join(runDir, "final-report.md"), report(m, current));
console.log(`final: ${baseline.total} -> ${current.total}`);
console.log(`run dir: ${runDir}`);
