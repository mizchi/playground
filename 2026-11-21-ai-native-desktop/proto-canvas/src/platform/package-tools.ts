// パッケージの glue を作る道具 (ホスト側)。jco の generate (wasm で動く) をホストで動かし、生成した JS を読み込む。
// パッケージからは .wasm と app.json しか受け取らないので、パッケージの JS がページの中で動くことはない。
import type { ComponentModule, PackageTools } from './packages.ts';

export function hostPackageTools(loadModule: (js: string) => Promise<ComponentModule>): PackageTools {
  return {
    async generate(component) {
      const { generate } = await import('@bytecodealliance/jco/component');
      const out = await generate(component, { name: 'app', instantiation: { tag: 'sync' }, noTypescript: true });
      return { files: out.files, imports: out.imports };
    },
    loadModule,
  };
}

/** ブラウザ: Blob の URL にして import する */
export const blobModule = async (js: string): Promise<ComponentModule> => {
  const url = URL.createObjectURL(new Blob([js], { type: 'text/javascript' }));
  try {
    return (await import(/* @vite-ignore */ url)) as ComponentModule;
  } finally {
    URL.revokeObjectURL(url);
  }
};
