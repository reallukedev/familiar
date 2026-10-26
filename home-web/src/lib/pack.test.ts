import { describe, expect, it } from 'vitest';
import { PACK_TEMPLATE, lineForField, lintManifest } from './pack';

const fields = (source: string) => lintManifest(source).map((issue) => issue.field);

describe('lintManifest', () => {
  it('accepts the example in the editor', () => {
    expect(lintManifest(PACK_TEMPLATE)).toEqual([]);
  });

  it('points at the id, tool name prefix, risk and timeout', () => {
    const source = PACK_TEMPLATE.replace('id = "things"', 'id = "Things!"').replace('risk = "read"', 'risk = "maybe"').replace('timeout_s = 30', 'timeout_s = 900');
    expect(fields(source)).toEqual(['id', 'tools[0].risk', 'tools[0].timeout_s']);
    const prefix = lintManifest(PACK_TEMPLATE.replace('name = "things_today"', 'name = "today"'));
    expect(prefix[0]).toMatchObject({ field: 'tools[0].name', line: 8, message: 'Tool names start with "things_".' });
  });

  it('requires a request_id on tools that send', () => {
    const send = PACK_TEMPLATE.replace('risk = "read"', 'risk = "send"');
    expect(fields(send)).toEqual(['tools[0].input_schema']);
  });

  it('does not read keys inside multi-line strings', () => {
    const tricky = PACK_TEMPLATE.replace("const args = JSON.parse(argv[0]);", 'risk = "nonsense"\n  id = "x"');
    expect(lintManifest(tricky)).toEqual([]);
  });

  it('finds duplicate tools and empty manifests', () => {
    const tool = PACK_TEMPLATE.slice(PACK_TEMPLATE.indexOf('[[tools]]'));
    expect(fields(PACK_TEMPLATE + '\n' + tool)).toContain('tools[1].name');
    expect(fields('  ')).toEqual(['manifest']);
  });
});

describe('lineForField', () => {
  it('maps the server’s field names back to lines', () => {
    expect(lineForField(PACK_TEMPLATE, 'id')).toBe(1);
    expect(lineForField(PACK_TEMPLATE, 'tools[0].script')).toBe(17);
    expect(lineForField(PACK_TEMPLATE, 'tools.0.risk')).toBe(11);
    expect(lineForField(PACK_TEMPLATE, 'tools[0]')).toBe(7);
    expect(lineForField(PACK_TEMPLATE, 'tools[3].name')).toBeNull();
    expect(lineForField(PACK_TEMPLATE, undefined)).toBeNull();
  });
});
