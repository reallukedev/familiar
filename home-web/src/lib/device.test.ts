import { describe, expect, it } from 'vitest';
import { deviceName, newDeviceSignIns } from './device';

const MAC_SAFARI = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Safari/605.1.15';
const IPHONE = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Mobile/15E148 Safari/604.1';
const WIN_CHROME = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36';

describe('deviceName', () => {
  it.each([
    [MAC_SAFARI, 'Safari on Mac'],
    [IPHONE, 'Safari on iPhone'],
    [WIN_CHROME, 'Chrome on Windows'],
    ['curl/8.4.0', 'A script'],
    [null, 'Unknown device'],
  ])('%s', (ua, name) => expect(deviceName(ua)).toBe(name));
});

describe('newDeviceSignIns', () => {
  it('flags sign-ins from a browser and network not seen before, but not the first one', () => {
    const events = [
      { id: '4', kind: 'sign_in', ip: '1.1.1.1', user_agent: MAC_SAFARI },
      { id: '3', kind: 'sign_in', ip: '2.2.2.2', user_agent: IPHONE },
      { id: '2', kind: 'sign_in_failed', ip: '9.9.9.9', user_agent: WIN_CHROME },
      { id: '1', kind: 'sign_in', ip: '1.1.1.1', user_agent: MAC_SAFARI },
    ];
    expect([...newDeviceSignIns(events)]).toEqual(['3']);
  });
});
