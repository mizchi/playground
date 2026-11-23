import type { ActionRequest, AppId, NodeId, TreeUpdate } from '../protocol.ts';

/** アプリは view を知らない。P1 を出し、P2 を受けるだけ。 */
export interface App {
  /** 窓の id (同じアプリを複数開くと、窓ごとに違う id になる) */
  id: AppId;
  start(emit: (u: TreeUpdate) => void): void;
  onAction(req: ActionRequest): void;
  /** 窓を閉じるとき (後片付け) */
  stop?(): void;
  /**
   * P2': 格子で描くノード (role log の端末の画面) に、窓の中で使える大きさを桁と行で知らせる。
   * px ではなく格子の単位で渡すので、アプリは字の大きさも窓の配置も知らなくてよい
   */
  onResize?(node: NodeId, cols: number, rows: number): void;
}
