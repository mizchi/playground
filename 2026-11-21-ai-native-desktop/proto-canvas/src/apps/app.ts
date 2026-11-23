import type { ActionRequest, AppId, TreeUpdate } from '../protocol.ts';

/** アプリは view を知らない。P1 を出し、P2 を受けるだけ。 */
export interface App {
  /** 窓の id (同じアプリを複数開くと、窓ごとに違う id になる) */
  id: AppId;
  start(emit: (u: TreeUpdate) => void): void;
  onAction(req: ActionRequest): void;
  /** 窓を閉じるとき (後片付け) */
  stop?(): void;
}
