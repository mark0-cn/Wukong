import { render, screen, within } from '@testing-library/react';
import { expect, test } from 'vitest';
import { EnvironmentHealth } from './EnvironmentHealth';

test('a configured environment with an offline daemon does not claim tool health', () => {
  render(<EnvironmentHealth health={{ configured: true, docker: false, running: null, tools: null, checked_at: '2026-11-11T00:00:00Z', tools_checked_at: null, image: null, checks: {}, error: null }} />);
  expect(screen.getByText('已配置')).toBeTruthy();
  expect(within(screen.getByText('Docker 连接').parentElement!).getByText('不可用 / 未通过')).toBeTruthy();
  expect(within(screen.getByText('工具自检').parentElement!).getByText('未检查')).toBeTruthy();
  expect(screen.getByText('技术详情').closest('details')?.open).toBe(false);
});

test('a workspace configuration change invalidates a previously successful observation', () => {
  render(<EnvironmentHealth epoch={2} health={{ epoch: 1, configured: true, docker: true, running: true, tools: true, checked_at: '2026-11-11T00:00:00Z', tools_checked_at: '2026-11-11T00:00:00Z', image: null, checks: {}, error: null }} />);
  expect(screen.queryByText('通过')).toBeNull();
  expect(screen.getAllByText('未检查')).toHaveLength(4);
});
