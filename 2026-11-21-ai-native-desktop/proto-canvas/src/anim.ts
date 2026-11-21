// ばねによるアニメーション。時刻 (ms) を外から渡すので決定的にテストできる。

export interface SpringConfig {
  stiffness: number;
  /** 1 = 臨界減衰 (行き過ぎない)。1 未満で少し行き過ぎて戻る */
  dampingRatio: number;
}

export const TILE_SPRING: SpringConfig = { stiffness: 420, dampingRatio: 1 };
export const PREVIEW_SPRING: SpringConfig = { stiffness: 700, dampingRatio: 0.82 };

export class Spring {
  x: number;
  v = 0;
  target: number;
  private cfg: SpringConfig;
  constructor(x: number, cfg: SpringConfig) {
    this.x = x;
    this.target = x;
    this.cfg = cfg;
  }
  /** dt 秒進める。true なら止まった */
  step(dt: number): boolean {
    const k = this.cfg.stiffness;
    const c = 2 * Math.sqrt(k) * this.cfg.dampingRatio;
    // 細かく刻んで安定させる (半陰的オイラー)
    let left = Math.min(dt, 0.064);
    while (left > 0) {
      const h = Math.min(left, 1 / 240);
      this.v += (-k * (this.x - this.target) - c * this.v) * h;
      this.x += this.v * h;
      left -= h;
    }
    if (Math.abs(this.x - this.target) < 0.05 && Math.abs(this.v) < 0.5) {
      this.x = this.target;
      this.v = 0;
      return true;
    }
    return false;
  }
  snap(): void {
    this.x = this.target;
    this.v = 0;
  }
}

/** 矩形と不透明度をまとめて動かす */
export class AnimatedRect {
  private s: Spring[];
  constructor(r: { x: number; y: number; w: number; h: number }, cfg: SpringConfig) {
    this.s = [r.x, r.y, r.w, r.h].map((v) => new Spring(v, cfg));
  }
  set target(r: { x: number; y: number; w: number; h: number }) {
    [r.x, r.y, r.w, r.h].forEach((v, i) => (this.s[i].target = v));
  }
  get value() {
    const [x, y, w, h] = this.s.map((s) => s.x);
    return { x, y, w, h };
  }
  step(dt: number): boolean {
    return this.s.map((s) => s.step(dt)).every(Boolean);
  }
  snap(): void {
    this.s.forEach((s) => s.snap());
  }
}
