import { render, screen } from '@testing-library/react';
import { expect, test } from 'vitest';
import { ImagePullProgress } from './ImagePullProgress';

test('shows layer readiness without presenting it as a byte percentage', () => {
  render(<ImagePullProgress version="0.2.0" busy value={{ layers: [
    { id: 'abcdef123456', state: 'ready' }, { id: '123456abcdef', state: 'downloaded' },
  ], elapsed_ms: 81000, finished: false, cached: false, truncated: false }} />);
  expect(screen.getByText('已就绪 1 层 / 已发现 2 层')).toBeTruthy();
  expect(screen.getByText('已用时 1:21')).toBeTruthy();
  expect(screen.getByRole('progressbar').hasAttribute('value')).toBe(false);
  expect(screen.getByText('已下载')).toBeTruthy();
});

test('local image reuse does not pretend it downloaded or passed environment validation', () => {
  render(<ImagePullProgress version="0.2.0" busy={false} value={{ layers: [], elapsed_ms: 0, finished: true, cached: true, truncated: false }} />);
  expect(screen.getByText('复用本地镜像，无需下载')).toBeTruthy();
  expect(screen.queryByRole('progressbar')).toBeNull();
  expect(screen.queryByText('已就绪')).toBeNull();
});
