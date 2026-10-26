import { describe, expect, it } from 'vitest';
import { base64UrlToBytes, pushSupported } from './push';

describe('push', () => {
  it('decodes an unpadded base64url VAPID key', () => {
    const key = base64UrlToBytes('BA1Hxzyi1RUM1b5wjxsn7nGxAszw2u61m164i3MrAIxHF6YK5h4SDYic-dRuU_RCPCfA5aq9ojSwk5Y2EmClBPs');
    expect(key.length).toBe(65);
    expect(key[0]).toBe(4);
    expect(Array.from(base64UrlToBytes('-_8'))).toEqual([0xfb, 0xff]);
  });

  it('reports no support where there is no service worker or Push API', () => {
    expect(pushSupported()).toBe(false);
  });
});
