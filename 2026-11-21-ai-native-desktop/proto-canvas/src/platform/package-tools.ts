// パッケージの glue を作る道具 (ホスト側)。jco の generate (wasm で動く) をホストで動かし、生成した JS を読み込む。
// パッケージからは .wasm と app.json しか受け取らないので、パッケージの JS がページの中で動くことはない。
import type { PackageTools } from './packages.ts';

export const hostPackageTools: PackageTools = {
  async generate(component) {
    const { generate } = await import('@bytecodealliance/jco/component');
    const out = await generate(component, { name: 'app', instantiation: { tag: 'sync' }, noTypescript: true });
    return { files: out.files, imports: out.imports };
  },
};
