type Waiting<V, R> = { value: V; resolve: (result: R | undefined) => void; reject: (cause: unknown) => void };
type Lane = { busy: boolean; waiting: Waiting<unknown, unknown> | null };

const lanes = new Map<string, Lane>();

/**
 * Send changes to one setting one at a time, so they can't land out of order.
 * While one is out, only the newest waiting value goes next; values it replaces
 * resolve with `undefined` without being sent. The last choice always wins.
 */
export function sendLatest<V, R>(key: string, value: V, send: (value: V) => Promise<R>): Promise<R | undefined> {
  let lane = lanes.get(key);
  if (!lane) lanes.set(key, (lane = { busy: false, waiting: null }));
  const current = lane;
  return new Promise<R | undefined>((resolve, reject) => {
    current.waiting?.resolve(undefined);
    current.waiting = { value, resolve, reject } as Waiting<unknown, unknown>;
    if (!current.busy) void drain(key, current, send as (value: unknown) => Promise<unknown>);
  });
}

async function drain(key: string, lane: Lane, send: (value: unknown) => Promise<unknown>) {
  lane.busy = true;
  while (lane.waiting) {
    const { value, resolve, reject } = lane.waiting;
    lane.waiting = null;
    try {
      resolve(await send(value));
    } catch (cause) {
      reject(cause);
    }
  }
  lane.busy = false;
  lanes.delete(key);
}
