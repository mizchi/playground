// Score -> ask Claude for N refactor candidates -> re-score / judge -> keep the best one if it passes the gates.
import { execFileSync, spawn } from "node:child_process";
import { cpSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
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
    // Generate N refactor candidates in parallel per iteration and keep the best one.
    "best-of": { type: "string", default: "1" },
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

const hash = (p: string, root = work) => createHash("sha256").update(readFileSync(join(root, p))).digest("hex");
const testHash = hash(TEST_FILE);

function claudeAsync(prompt: string, cwd = work, tools = ""): Promise<string> {
  return new Promise((done) => {
    const extra = tools ? ["--permission-mode", "acceptEdits"] : [];
    const p = spawn("claude", ["-p", prompt, "--model", args.model!, "--allowedTools", tools, ...extra], {
      cwd,
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
  votes?: string[];
  cognitive: { sum: number; max: number; functions: number };
  candidates?: { k: number; score: number; votes?: string[]; reason: string; cognitive: object }[];
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
const clean = (t: string) => t.replace(/^Warning: no stdin.*\n/m, "").trim().slice(0, 1200);

let m = collect(work);
const m0 = m;
let current = score(m);
const baseline = current;
console.log(`baseline: ${current.total}`);
console.log(report(m, current));

type Candidate = {
  k: number;
  dir: string;
  metrics: ReturnType<typeof collect>;
  score: Score;
  diff: string;
  reason: string;
  votes?: string[];
  reviews: string[];
};

const cognitiveOf = (mm: ReturnType<typeof collect>) => ({
  sum: mm.functions.reduce((n, f) => n + f.cognitive, 0),
  max: Math.max(...mm.functions.map((f) => f.cognitive)),
  functions: mm.functions.length,
});
const betterCount = (c: Candidate) => (c.votes ?? []).filter((v) => v === "better").length;
const TOOLS = "Read Edit Write Glob Grep Bash(node --test:*)";
const N = Number(args["best-of"]);

for (let i = 1; i <= Number(args.iterations); i++) {
  const prompt = refactorPrompt(report(m, current), args.feedback ? feedback : []);
  writeFileSync(join(runDir, `prompt-${i}.md`), prompt);

  // 1. N candidates, each in its own clone of the current accepted state.
  const dirs = Array.from({ length: N }, (_, k) => join(runDir, `cand-${i}-${k + 1}`));
  for (const d of dirs) execFileSync("git", ["clone", "-q", work, d]);
  const logs = await Promise.all(dirs.map((d) => claudeAsync(prompt, d, TOOLS)));
  logs.forEach((l, k) => writeFileSync(join(runDir, `claude-${i}-${k + 1}.log`), l));

  // 2. Score and gate every candidate.
  const cands: Candidate[] = dirs.map((dir, k) => {
    const metrics = collect(dir);
    const sc = score(metrics);
    const diff = execFileSync("git", ["diff", "HEAD", "--", "src"], { cwd: dir, encoding: "utf8" });
    let reason = "";
    if (hash(TEST_FILE, dir) !== testHash) reason = "test file modified";
    else if (!sc.testsPassed) reason = "tests failed";
    else if (!diff) reason = "no change";
    else if (args.accept === "score" ? sc.total <= current.total : sc.total < current.total)
      reason = args.accept === "score" ? "score did not improve" : "score dropped";
    return { k: k + 1, dir, metrics, score: sc, diff, reason, reviews: [] };
  });

  // 3. Judge the survivors (all judges of all candidates run in parallel).
  if (args.accept === "judge") {
    await Promise.all(
      cands
        .filter((c) => !c.reason)
        .map(async (c) => {
          const j = await judgeVotes(c.diff, Number(args.judges));
          c.votes = j.votes;
          c.reviews = j.texts.map(clean);
          j.texts.forEach((t, n) => writeFileSync(join(runDir, `judge-${i}-${c.k}-${n + 1}.md`), t));
          if (j.verdict !== "better") c.reason = `judge: ${j.votes.join("/")}`;
        }),
    );
  }

  // 4. Pick: most "better" votes, then higher score, then lower total cognitive complexity.
  const ranked = [...cands].sort(
    (a, b) =>
      Number(!!a.reason) - Number(!!b.reason) ||
      betterCount(b) - betterCount(a) ||
      b.score.total - a.score.total ||
      cognitiveOf(a.metrics).sum - cognitiveOf(b.metrics).sum,
  );
  const best = ranked[0];
  const accepted = !best.reason;
  const summaryLine = cands
    .map((c) => `#${c.k} ${c.score.total}${c.votes ? ` ${c.votes.join("/")}` : ""}${c.reason ? ` (${c.reason})` : ""}`)
    .join(" | ");
  console.log(`iteration ${i}: ${summaryLine} => ${accepted ? `ACCEPT #${best.k} (${current.total} -> ${best.score.total})` : "REJECT all"}`);

  history.push({
    iteration: i,
    before: current,
    after: best.score,
    accepted,
    reason: accepted ? `accepted candidate ${best.k}` : "all candidates rejected",
    votes: best.votes,
    cognitive: cognitiveOf(best.metrics),
    candidates: cands.map((c) => ({
      k: c.k,
      score: c.score.total,
      votes: c.votes,
      reason: c.reason || "ok",
      cognitive: cognitiveOf(c.metrics),
    })),
  });

  if (accepted) {
    rmSync(join(work, "src"), { recursive: true, force: true });
    cpSync(join(best.dir, "src"), join(work, "src"), { recursive: true });
    git("add", "-A");
    git("-c", "user.name=loop", "-c", "user.email=loop@localhost", "commit", "-q", "-m", `iteration ${i} (candidate ${best.k})`);
    m = best.metrics;
    current = best.score;
  } else {
    // Only when every candidate failed: one feedback entry per iteration, covering all candidates.
    const failed = cands.filter((c) => c.reason !== "no change");
    if (failed.length) {
      const entry = failed
        .map((c) => {
          const body = c.reviews.length
            ? c.reviews.map((r, n) => `Reviewer ${n + 1}:\n${r}`).join("\n\n")
            : "(no reviewer comments)";
          return `#### Candidate ${c.k} — ${c.reason}\n\n${body}`;
        })
        .join("\n\n");
      feedback.push(entry);
      if (feedback.length > MAX_FEEDBACK) feedback.shift();
    }
  }
  for (const c of cands) rmSync(c.dir, { recursive: true, force: true });
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
const summary = { fixture: args.fixture, model: args.model, accept: args.accept, judges: Number(args.judges), feedback: args.feedback, bestOf: N, baseline, baseCognitive, final: current, history, judge };
writeFileSync(join(runDir, "summary.json"), JSON.stringify(summary, null, 2));
writeFileSync(join(runDir, "final-report.md"), report(m, current));
console.log(`final: ${baseline.total} -> ${current.total}`);
console.log(`run dir: ${runDir}`);
