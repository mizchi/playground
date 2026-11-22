// web-canvas backend の a11y: 意味ツリーを ARIA 付きの見えない DOM (ミラー) に写す。
// <canvas> はスクリーンリーダーから見えないので、意味ツリー (正) からこの DOM を作る。
// 各要素は Canvas 上の widget の位置に重ねる (VoiceOver のカーソルの枠が合うように)。
// スクリーンリーダーからの操作 (VO+Space など) は click として届くので、人の意味的な操作として shell に返す。
import type { AppId, NodeId, Rect, SemanticNode } from '../protocol.ts';
import type { Shell } from '../shell.ts';

type Model = ReturnType<Shell['accessibility']>;

const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]!);

export interface AriaMirror {
  update(model: Model): void;
  announce(text: string): void;
}

export function createAriaMirror(container: HTMLElement, activate: (app: AppId, node: NodeId) => void, restore: (app: AppId) => void): AriaMirror {
  container.innerHTML = '<div class="tree"></div><div class="live" aria-live="polite" role="status"></div>';
  const tree = container.querySelector('.tree') as HTMLElement;
  const live = container.querySelector('.live') as HTMLElement;
  let last = '';

  container.addEventListener('click', (e) => {
    const el = (e.target as HTMLElement).closest<HTMLElement>('[data-app]');
    if (!el) return;
    if (el.dataset.restore) restore(el.dataset.app!);
    else if (el.dataset.node) activate(el.dataset.app!, el.dataset.node);
  });

  const pos = (r: Rect | null | undefined) => (r ? ` style="left:${r.x}px;top:${r.y}px;width:${r.w}px;height:${r.h}px"` : '');

  const node = (app: AppId, n: SemanticNode, byId: Map<NodeId, SemanticNode>, rects: Model['rects']): string => {
    const kids = (n.children ?? []).map((c) => byId.get(c)).filter((c): c is SemanticNode => !!c);
    const inner = kids.map((c) => node(app, c, byId, rects)).join('');
    const label = esc(n.label ?? '');
    const at = `data-app="${esc(app)}" data-node="${esc(n.id)}" id="a11y-${esc(app)}-${esc(n.id)}"${pos(rects.get(`${app}/${n.id}`))}`;
    const dis = n.disabled ? ' aria-disabled="true"' : '';
    // 操作できるのに操作の役割を持たないもの (クリックできる画像など) は、ボタンとして出す
    if (n.actions?.includes('click') && ['img', 'cell', 'paragraph', 'heading', 'columnheader', 'progressbar'].includes(n.role))
      return `<div role="button" tabindex="-1" aria-label="${label}" ${at}${dis}>${inner}</div>`;
    switch (n.role) {
      case 'x-window':
        return `<section role="region" aria-label="${label}" data-app="${esc(app)}">${inner}</section>`;
      case 'heading':
        return `<h2 ${at}>${label}</h2>`;
      case 'paragraph':
        return `<p ${at}>${label}</p>`;
      case 'list':
        return `<ul role="list" aria-label="${label}">${kids.map((c) => `<li>${node(app, c, byId, rects)}</li>`).join('')}</ul>`;
      case 'listitem':
      case 'group':
        return `<div role="group" aria-label="${label}">${inner}</div>`;
      case 'checkbox':
      case 'switch':
        return `<div role="${n.role}" aria-checked="${n.checked === 'mixed' ? 'mixed' : n.checked ? 'true' : 'false'}" tabindex="-1" ${at}${dis}>${label}</div>`;
      case 'textbox':
        return `<div role="textbox" aria-label="${label}" aria-multiline="${n.multiline ? 'true' : 'false'}" tabindex="-1" ${at}${dis}>${esc(n.value ?? '')}</div>`;
      case 'button':
        return `<div role="button" tabindex="-1" ${at}${dis}>${label}</div>`;
      case 'radio':
        return `<div role="radio" aria-checked="${n.checked ? 'true' : 'false'}" tabindex="-1" ${at}${dis}>${label}</div>`;
      case 'combobox':
        return `<div role="combobox" aria-expanded="false" aria-label="${label}" tabindex="-1" ${at}${dis}>${esc(n.value ?? '')}</div>`;
      case 'slider':
      case 'spinbutton':
      case 'progressbar': {
        const v = `aria-valuenow="${n.valueNow ?? 0}"${n.valueMin !== undefined ? ` aria-valuemin="${n.valueMin}"` : ''}${n.valueMax !== undefined ? ` aria-valuemax="${n.valueMax}"` : ''}`;
        return `<div role="${n.role}" aria-label="${label}" ${v} tabindex="-1" ${at}${dis}></div>`;
      }
      case 'tablist':
        return `<div role="tablist" aria-label="${label}">${inner}</div>`;
      case 'tab':
        return `<div role="tab" aria-selected="${n.selected ? 'true' : 'false'}" tabindex="-1" ${at}${dis}>${label}</div>`;
      case 'table':
        return `<div role="table" aria-label="${label}">${inner}</div>`;
      case 'row':
        return `<div role="row">${inner}</div>`;
      case 'cell':
        return `<div role="cell" ${at}>${label}</div>`;
      case 'columnheader':
        return `<div role="columnheader" ${at}>${label}</div>`;
      case 'separator':
        return '<div role="separator"></div>';
      case 'img':
        return `<div role="img" aria-label="${label}" ${at}></div>`;
    }
  };

  return {
    update(model) {
      const html =
        model.windows
          .map((w) => {
            const byId = new Map(w.nodes.map((n) => [n.id, n] as const));
            const root = w.root ? byId.get(w.root) : undefined;
            return root ? node(w.app, root, byId, model.rects) : '';
          })
          .join('') +
        (model.minimized.length
          ? `<nav aria-label="ドック">${model.minimized.map((m) => `<div role="button" tabindex="-1" data-app="${esc(m.app)}" data-restore="1"${pos(m.rect)}>${esc(m.title)} を元に戻す</div>`).join('')}</nav>`
          : '');
      // 変わったときだけ差し替える (読み上げ位置を不用意に失わないため)
      if (html !== last) {
        tree.innerHTML = html;
        last = html;
      }
    },
    announce(text) {
      live.textContent = '';
      // 同じ文でも読み上げ直されるよう、いったん空にしてから入れる
      requestAnimationFrame(() => (live.textContent = text));
    },
  };
}
