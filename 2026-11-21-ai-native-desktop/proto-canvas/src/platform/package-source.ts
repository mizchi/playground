// ブラウザでパッケージを読む: app.json の URL と、その隣の .wasm。
// パッケージの JS は動かさない (glue はホストが .wasm から作る) ので、ほかの origin のパッケージも入れられる (CORS が要る)。
import type { PackageSource } from './packages.ts';

export function urlPackageSource(url: string, base = location.href): PackageSource | string {
  let u: URL;
  try {
    u = new URL(url, base);
  } catch {
    return 'URL ではない';
  }
  if (u.protocol !== 'https:' && u.protocol !== 'http:') return 'http(s) の URL だけ';
  const get = async (href: string) => {
    const r = await fetch(href, { credentials: 'omit' });
    if (!r.ok) throw new Error(`${href}: ${r.status}`);
    return r;
  };
  return {
    manifest: async () => (await get(u.href)).json(),
    bytes: async (p) => new Uint8Array(await (await get(new URL(p, u).href)).arrayBuffer()),
  };
}
