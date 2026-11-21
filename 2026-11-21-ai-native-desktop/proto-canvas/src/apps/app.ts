import type { ActionRequest, AppId, TreeUpdate } from '../protocol.ts';

/** アプリは view を知らない。P1 を出し、P2 を受けるだけ。 */
export interface App {
  id: AppId;
  start(emit: (u: TreeUpdate) => void): void;
  onAction(req: ActionRequest): void;
}
