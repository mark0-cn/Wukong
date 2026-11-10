import { describe, expect, it } from 'vitest';
import { negotiate, text, message } from './index';
describe('localization boundary', () => {
  it('negotiates supported scripts and falls back to English', () => {
    expect(negotiate('zh_Hans_CN')).toBe('zh-CN');
    expect(negotiate('zh-SG')).toBe('zh-CN');
    expect(negotiate('fr-FR')).toBe('en-US');
    expect(negotiate('zh-Hant')).toBe('en-US');
  });
  it('keeps unknown diagnostics and raw evidence intact', () => {
    const detail = '原始字节 SHA256 abc123\n0x42';
    expect(message({ code: 'unknown-error', args: {}, detail })).toContain(detail);
    expect(message(detail)).toBe(detail);
    expect(text('unknown-tool', {}, 'custom_tool')).toBe('custom_tool');
  });
});
