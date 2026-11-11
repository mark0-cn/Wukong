import { expect, test } from 'vitest';
import { displayPath } from './displayPath';

test('formats Windows drive and UNC paths without changing other namespaces or Linux paths', () => {
  expect(displayPath(String.raw`\\?\C:\Users\test\WukongData\.wukong`)).toBe(String.raw`C:\Users\test\WukongData\.wukong`);
  expect(displayPath(String.raw`\\?\UNC\server\share\tasks`)).toBe(String.raw`\\server\share\tasks`);
  for (const path of [String.raw`\\?\Volume{abc}\data`, String.raw`\\.\pipe\docker`, '/home/user/.local/share/Wukong', String.raw`D:\data`]) {
    expect(displayPath(path)).toBe(path);
  }
});
