// 配色の token の値 (ライト / ダーク)。正はここ。www/index.html の CSS 変数と、veriss への lowering が同じ値を使う
// (test/theme.test.ts が index.html との一致と、両方のテーマでのコントラストを veriss で確かめる)。
import type { ColorToken } from './protocol.ts';

export type ThemeName = 'light' | 'dark';
export type Palette = Record<ColorToken, string>;

export const THEMES: Record<ThemeName, Palette> = {
  light: {
    bg: '#eef0f4',
    surface: '#ffffff',
    text: '#1d2230',
    muted: '#5d6475',
    accent: '#3557d4',
    accentText: '#ffffff',
    border: '#c9ced9',
    danger: '#c2362f',
    ghost: '#b4529a',
  },
  dark: {
    bg: '#15171d',
    surface: '#1f222b',
    text: '#e6e8ef',
    muted: '#9aa1b2',
    accent: '#7d97ff',
    accentText: '#10131a',
    border: '#3a3f4d',
    danger: '#ff7a70',
    ghost: '#e48ad0',
  },
};

/**
 * shell 自身の UI (タイトルバー・ズームの札・提案の番号・ドラッグ中の札・ドック) が使う「文字色 on 背景色」の組。
 * アプリの view は lowering して veriss で検査するが、これらは shell が直接描くので、この表を検査用の見本にする。
 * shell.ts / render.ts の text の color と、その下に塗る rect の fill の組を写したもの。
 */
export const CHROME_PAIRS: { name: string; fg: ColorToken; bg: ColorToken; size: number; weight: number }[] = [
  { name: 'タイトルバー (フォーカスなし)', fg: 'muted', bg: 'surface', size: 12, weight: 600 },
  { name: 'タブ (選択)', fg: 'text', bg: 'bg', size: 12, weight: 600 },
  { name: 'タブ (非選択)', fg: 'muted', bg: 'surface', size: 12, weight: 600 },
  { name: 'ズーム中の札', fg: 'accentText', bg: 'accent', size: 11, weight: 600 },
  { name: 'はみ出しの札', fg: 'accentText', bg: 'danger', size: 11, weight: 600 },
  { name: 'ドックの札', fg: 'text', bg: 'bg', size: 12, weight: 600 },
  { name: 'ドラッグ中の窓の札', fg: 'text', bg: 'surface', size: 12, weight: 600 },
  { name: '落とす先の説明 / 提案の札', fg: 'accentText', bg: 'ghost', size: 12, weight: 600 },
  { name: '提案の手順番号', fg: 'accentText', bg: 'ghost', size: 11, weight: 700 },
  { name: 'ボタン', fg: 'accentText', bg: 'accent', size: 14, weight: 600 },
  { name: 'sensitive なボタン', fg: 'accentText', bg: 'danger', size: 14, weight: 600 },
  { name: 'チェックの印', fg: 'accentText', bg: 'accent', size: 13, weight: 700 },
  { name: 'IME の変換中の文字', fg: 'accent', bg: 'surface', size: 14, weight: 400 },
  { name: '通知の出どころ', fg: 'muted', bg: 'surface', size: 12, weight: 600 },
  { name: '通知の題名', fg: 'text', bg: 'surface', size: 14, weight: 600 },
  { name: '通知の本文', fg: 'text', bg: 'surface', size: 13, weight: 400 },
  { name: '本文', fg: 'text', bg: 'surface', size: 14, weight: 400 },
  { name: '補足の文字', fg: 'muted', bg: 'surface', size: 13, weight: 400 },
];
