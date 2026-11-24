// 合成 URL データ。ブロックリスト (= Bloom filter に入れる鍵集合) は「怪しい URL」、
// 問い合わせの大半は「普通の URL」という、Safe Browsing 的な状況を模している。
// 現実と同じく完全には分離できないよう、両側に紛らわしいものを混ぜている。
import type { Rng } from "./nn.ts";

const pick = <T>(rng: Rng, xs: readonly T[]): T => xs[Math.floor(rng() * xs.length)];
const int = (rng: Rng, lo: number, hi: number) => lo + Math.floor(rng() * (hi - lo + 1));
const ALNUM = "abcdefghijklmnopqrstuvwxyz0123456789";
const token = (rng: Rng, n: number) => Array.from({ length: n }, () => pick(rng, ALNUM)).join("");

const WORDS = [
  "news", "blog", "shop", "cloud", "photo", "music", "travel", "food", "game", "tech", "sports", "book",
  "garden", "health", "movie", "code", "design", "school", "city", "market", "home", "pet", "car", "art",
  "docs", "wiki", "forum", "jobs", "weather", "maps", "video", "radio", "kitchen", "craft", "science",
];
const BRANDS = ["paypal", "apple", "amazon", "bank", "netflix", "google", "microsoft", "wallet", "post", "card"];
const BAIT = ["secure", "login", "verify", "account", "update", "support", "signin", "confirm"];
const GOOD_TLD = [".com", ".org", ".net", ".jp", ".io", ".dev", ".co.uk"];
const BAD_TLD = [".xyz", ".top", ".info", ".com", ".ru", ".click", ".shop", ".net"];

export function benignUrl(rng: Rng): string {
  const host = (rng() < 0.5 ? "www." : "") + pick(rng, WORDS) + (rng() < 0.3 ? pick(rng, WORDS) : "") + pick(rng, GOOD_TLD);
  const depth = int(rng, 0, 3);
  const path = Array.from({ length: depth }, () => (rng() < 0.8 ? pick(rng, WORDS) : String(int(rng, 1, 9999))));
  let u = `https://${host}/${path.join("/")}`;
  if (rng() < 0.03) u += `/${pick(rng, BAIT)}`; // 正規サイトのログインページ
  if (rng() < 0.15) u += `?id=${int(rng, 1, 99999)}`;
  return u;
}

export function maliciousUrl(rng: Rng): string {
  const r = rng();
  if (r < 0.15) {
    // 乗っ取られた普通のサイト: 見た目はほぼ benign
    return `https://${pick(rng, WORDS)}${pick(rng, GOOD_TLD)}/${pick(rng, WORDS)}/${token(rng, int(rng, 4, 8))}`;
  }
  const scheme = rng() < 0.7 ? "http" : "https";
  const sep = pick(rng, ["-", ".", ""]);
  const host = r < 0.6
    ? `${pick(rng, BRANDS)}${sep}${pick(rng, BAIT)}${rng() < 0.5 ? int(rng, 1, 999) : ""}${pick(rng, BAD_TLD)}`
    : `${token(rng, int(rng, 6, 14))}${pick(rng, BAD_TLD)}`;
  let u = `${scheme}://${host}/${rng() < 0.5 ? pick(rng, BAIT) + "/" : ""}${token(rng, int(rng, 6, 16))}`;
  if (rng() < 0.4) u += pick(rng, [".php", ".html", "/index.php"]);
  if (rng() < 0.3) u += `?session=${token(rng, 10)}`;
  return u;
}

/** 重複無しで n 個生成。 */
export function uniqueSet(rng: Rng, n: number, gen: (rng: Rng) => string, exclude?: Set<string>): string[] {
  const s = new Set<string>();
  while (s.size < n) {
    const u = gen(rng);
    if (!exclude?.has(u)) s.add(u);
  }
  return [...s];
}
