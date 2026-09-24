// Collect quantitative metrics (cccc + similarity-ts + tests) and fold them into a 0-100 score.
import { execFileSync, spawnSync } from "node:child_process";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";

export type FnMetric = { path: string; name: string; line: number; cognitive: number; cyclomatic: number };
export type DupGroup = { similarity: number; members: { at: string; name: string; lines: number }[] };
export type Metrics = {
  loc: number;
  functions: FnMetric[];
  duplicates: DupGroup[];
  testsPassed: boolean;
};
export type Score = {
  total: number;
  complexity: number;
  duplication: number;
  complexityPenalty: number;
  duplicatedLines: number;
  testsPassed: boolean;
};

// Thresholds / weights. Deliberately simple; tune them as part of the experiment.
export const CONFIG = {
  cognitiveThreshold: 8,
  cyclomaticThreshold: 8,
  complexityDecay: 20, // complexity score = 100 * exp(-penalty / decay)
  similarityThreshold: 0.8,
  weights: { complexity: 0.5, duplication: 0.5 },
};

function listSources(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    if (name === "node_modules" || name.startsWith(".")) continue;
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...listSources(p));
    else if (/\.(ts|tsx)$/.test(name) && !/\.test\.tsx?$/.test(name)) out.push(p);
  }
  return out;
}

function runCccc(root: string, files: string[]): FnMetric[] {
  const json = JSON.parse(
    execFileSync("cccc", ["--no-config", ...files.map((f) => relative(root, f))], { encoding: "utf8", cwd: root }),
  );
  const fns: FnMetric[] = [];
  for (const file of json.files ?? []) {
    for (const f of file.functions ?? []) {
      fns.push({
        path: file.path,
        name: f.name,
        line: f.line,
        cognitive: f.cognitive,
        cyclomatic: f.cyclomatic,
      });
    }
  }
  return fns;
}

function runSimilarity(root: string, files: string[]): DupGroup[] {
  const res = spawnSync(
    "similarity-ts",
    ["--threshold", String(CONFIG.similarityThreshold), "--no-types", ...files.map((f) => relative(root, f))],
    { encoding: "utf8", cwd: root },
  );
  const text = (res.stdout ?? "").replace(/\x1b\[[0-9;]*m/g, "");
  // Two block shapes, each followed by indented "path:start-end name" lines:
  //   "Similarity: 98.50%, Score: ..."                                   (a pair)
  //   "Cluster 1: 4 functions, 4 pairwise matches, avg similarity 79.84%" (a cluster)
  const groups: DupGroup[] = [];
  let cur: DupGroup | undefined;
  for (const line of text.split("\n")) {
    const head = line.match(/^Similarity:\s*([\d.]+)%/) ?? line.match(/^Cluster \d+:.*avg similarity ([\d.]+)%/);
    if (head) {
      cur = { similarity: Number(head[1]) / 100, members: [] };
      groups.push(cur);
      continue;
    }
    const loc = line.match(/^\s+(\S+):(\d+)-(\d+)\s+(.+)$/);
    if (cur && loc) {
      cur.members.push({ at: `${loc[1]}:${loc[2]}-${loc[3]}`, name: loc[4].trim(), lines: Number(loc[3]) - Number(loc[2]) + 1 });
    } else if (line.trim() === "") {
      cur = undefined;
    }
  }
  return groups.filter((g) => g.members.length >= 2);
}

// Keeping the largest member is fine; everything else in the group is considered duplicated.
export function duplicatedLines(g: DupGroup): number {
  const sizes = g.members.map((m) => m.lines);
  return g.similarity * (sizes.reduce((a, b) => a + b, 0) - Math.max(...sizes));
}

function runTests(root: string): boolean {
  const res = spawnSync("node", ["--test", "src/order.test.ts"], { cwd: root, encoding: "utf8" });
  return res.status === 0;
}

export function collect(root: string): Metrics {
  const files = listSources(join(root, "src"));
  const loc = files.reduce((n, f) => n + readFileSync(f, "utf8").split("\n").filter((l) => l.trim()).length, 0);
  return {
    loc,
    functions: runCccc(root, files),
    duplicates: runSimilarity(root, files),
    testsPassed: runTests(root),
  };
}

export function score(m: Metrics): Score {
  const complexityPenalty = m.functions.reduce(
    (p, f) =>
      p +
      Math.max(0, f.cognitive - CONFIG.cognitiveThreshold) +
      0.5 * Math.max(0, f.cyclomatic - CONFIG.cyclomaticThreshold),
    0,
  );
  const complexity = 100 * Math.exp(-complexityPenalty / CONFIG.complexityDecay);
  const dupLines = m.duplicates.reduce((n, g) => n + duplicatedLines(g), 0);
  const duplication = 100 * Math.max(0, 1 - dupLines / Math.max(1, m.loc));
  const total = m.testsPassed
    ? CONFIG.weights.complexity * complexity + CONFIG.weights.duplication * duplication
    : 0;
  const r = (x: number) => Math.round(x * 10) / 10;
  return {
    total: r(total),
    complexity: r(complexity),
    duplication: r(duplication),
    complexityPenalty: r(complexityPenalty),
    duplicatedLines: r(dupLines),
    testsPassed: m.testsPassed,
  };
}

// Human/AI readable report: the worst offenders first.
export function report(m: Metrics, s: Score): string {
  const hot = m.functions
    .filter((f) => f.cognitive > CONFIG.cognitiveThreshold || f.cyclomatic > CONFIG.cyclomaticThreshold)
    .sort((a, b) => b.cognitive - a.cognitive);
  return [
    `score: ${s.total} (complexity ${s.complexity}, duplication ${s.duplication}, tests ${s.testsPassed ? "pass" : "FAIL"})`,
    `loc: ${m.loc}, functions: ${m.functions.length}`,
    ``,
    `## Complex functions (cognitive > ${CONFIG.cognitiveThreshold} or cyclomatic > ${CONFIG.cyclomaticThreshold})`,
    ...(hot.length ? hot.map((f) => `- ${f.path}:${f.line} ${f.name} cognitive=${f.cognitive} cyclomatic=${f.cyclomatic}`) : ["(none)"]),
    ``,
    `## Similar functions (similarity >= ${CONFIG.similarityThreshold})`,
    ...(m.duplicates.length
      ? m.duplicates.map(
          (g) => `- ${(g.similarity * 100).toFixed(1)}%: ${g.members.map((x) => `${x.at} ${x.name}`).join(" <-> ")}`,
        )
      : ["(none)"]),
  ].join("\n");
}

if (import.meta.main) {
  const root = process.argv[2] ?? "fixtures/order-service";
  const m = collect(root);
  const s = score(m);
  if (process.argv.includes("--json")) console.log(JSON.stringify({ score: s, metrics: m }, null, 2));
  else console.log(report(m, s));
}
