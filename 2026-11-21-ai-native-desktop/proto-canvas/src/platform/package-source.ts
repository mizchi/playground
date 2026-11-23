// ブラウザでパッケージを読む: app.json の URL から、同じ origin のファイルだけを読む。
// jco の生成物 (todo.js) はページの中で動く JS なので、能力の境界の外にある。出所を同じ origin に限る
// (本番では、ホストが .wasm から glue を作り、パッケージの JS は動かさない)。
import type { ComponentModule, PackageSource } from './packages.ts';

export function urlPackageSource(url: string, base = location.href): PackageSource | string {
  let u: URL;
  try {
    u = new URL(url, base);
  } catch {
    return 'URL ではない';
  }
  if (u.origin !== new URL(base).origin) return `同じ origin (${new URL(base).origin}) のパッケージだけを入れられる`;
  const at = (p: string) => new URL(p, u).href;
  return {
    manifest: async () => {
      const r = await fetch(u.href);
      if (!r.ok) throw new Error(`${r.status}`);
      return r.json();
    },
    module: (p) => import(/* @vite-ignore */ at(p)) as Promise<ComponentModule>,
    bytes: async (p) => {
      const r = await fetch(at(p));
      if (!r.ok) throw new Error(`${p}: ${r.status}`);
      return r.arrayBuffer();
    },
  };
}
