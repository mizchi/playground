// ブラウザでパッケージを読む: app.json の URL と、その隣の .wasm。
// パッケージの JS は動かさない (glue はホストが .wasm から作る) ので、ほかの origin のパッケージも入れられる (CORS が要る)。
import type { PackageSource } from './packages.ts';

/** 1 つのファイルを読む期限 (接続から本文を読み終えるまで)。応答しないサーバで起動が止まらないように */
export const PACKAGE_FETCH_TIMEOUT_MS = 10_000;

export function urlPackageSource(url: string, base = location.href, timeoutMs = PACKAGE_FETCH_TIMEOUT_MS): PackageSource | string {
  let u: URL;
  try {
    u = new URL(url, base);
  } catch {
    return 'URL ではない';
  }
  if (u.protocol !== 'https:' && u.protocol !== 'http:') return 'http(s) の URL だけ';
  // 同じ signal が本文の読み込みも止める (期限を過ぎると json() / arrayBuffer() も TimeoutError で失敗する)
  const read = async <T>(href: string, body: (r: Response) => Promise<T>): Promise<T> => {
    const signal = AbortSignal.timeout(timeoutMs);
    try {
      const r = await fetch(href, { credentials: 'omit', signal });
      if (!r.ok) throw new Error(`${href}: ${r.status}`);
      return await body(r);
    } catch (e) {
      if (signal.aborted) throw new Error(`${href}: ${timeoutMs / 1000} 秒で読み終わらない`);
      throw e;
    }
  };
  return {
    manifest: () => read(u.href, (r) => r.json()),
    bytes: (p) => read(new URL(p, u).href, async (r) => new Uint8Array(await r.arrayBuffer())),
  };
}
