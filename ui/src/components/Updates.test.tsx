import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';
import { Updates, type UpdateStatus } from './Updates';
import { api } from '../bridge';
vi.mock('../bridge', () => ({ api: vi.fn(), errorText: (e: unknown) => String(e) }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }));
const status: UpdateStatus = { version: '0.2.0', phase: 'idle', message: '', available: '0.2.1', notes: '', downloaded: 0, total: null, prepared: null, recovery: null, needs_setup: false, pending: false };
beforeEach(() => { vi.mocked(api<'update_action'>).mockReset(); });
test('cannot install until preparation succeeds and the user confirms restart', async () => {
  vi.mocked(api<'update_action'>).mockImplementation(async (_name, args) => args?.action === 'prepare' ? { ...status, prepared: '0.2.1', phase: 'ready' } : status);
  render(<Updates epoch={1} onClose={() => {}} onChanged={() => {}} />);
  await screen.findByText('0.2.0');
  const install = screen.getByRole('button', { name: '安装并重启' }) as HTMLButtonElement;
  expect(install.disabled).toBe(true);
  await userEvent.click(screen.getByRole('button', { name: '下载并验证更新' }));
  await waitFor(() => expect(install.disabled).toBe(false));
  await userEvent.click(install);
  expect(vi.mocked(api<'update_action'>).mock.calls.some(([, args]) => args?.action === 'install')).toBe(false);
  await userEvent.click(screen.getByRole('button', { name: /^确认$/ }));
  expect(vi.mocked(api<'update_action'>).mock.calls.some(([, args]) => args?.action === 'install')).toBe(true);
});
test('first launch explains Docker setup and pending recovery blocks installation', async () => {
  vi.mocked(api<'update_action'>).mockResolvedValue({ ...status, needs_setup: true, pending: true });
  render(<Updates epoch={1} onClose={() => {}} onChanged={() => {}} />);
  await screen.findByText('欢迎使用 Wukong · 准备环境');
  expect(screen.getByText(/安装并启动 Docker/)).toBeTruthy();
  expect((screen.getByRole('button', { name: '准备工具环境' }) as HTMLButtonElement).disabled).toBe(true);
});
