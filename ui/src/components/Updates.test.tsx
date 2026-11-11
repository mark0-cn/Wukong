import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';
import { Updates, type UpdateStatus } from './Updates';
import { api } from '../bridge';
vi.mock('../bridge', async () => ({ ...await vi.importActual('../bridge'), api: vi.fn() }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }));
const status: UpdateStatus = { version: '0.2.0', phase: 'idle', message: '', available: '0.2.1', notes: '', downloaded: 0, total: null, prepared: null, recovery: null, needs_setup: false, pending: false };
beforeEach(() => { vi.mocked(api<'update_action'>).mockReset(); });
test('cannot install until preparation succeeds and the user confirms restart', async () => {
  vi.mocked(api<'update_action'>).mockImplementation(async (_name, args) => args?.action === 'prepare' ? { ...status, prepared: '0.2.1', phase: 'ready' } : status);
  render(<Updates epoch={1} onClose={() => {}} onChanged={() => {}} />);
  await screen.findByText('0.2.0');
  expect(screen.queryByRole('button', { name: '安装并重启' })).toBeNull();
  await userEvent.click(screen.getByRole('button', { name: '下载并验证更新' }));
  const install = await screen.findByRole('button', { name: '安装并重启' }) as HTMLButtonElement;
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

test('setup separates readiness from client update status and hides maintenance', async () => {
  vi.mocked(api<'update_action'>).mockResolvedValue({ ...status, needs_setup: true, message: '当前已是最新版本' });
  render(<Updates epoch={1} onClose={() => {}} onChanged={() => {}} />);
  await screen.findByText('待准备');
  expect(screen.queryByText('当前已是最新版本')).toBeNull();
  expect(screen.queryByText('维护选项')).toBeNull();
  expect(screen.queryByRole('button', { name: '检查更新' })).toBeNull();
  expect(screen.getByRole('list', { name: '环境准备步骤' }).children).toHaveLength(3);
});

test('Docker failure appears once, diagnostic details can be read, and preparation can retry', async () => {
  const failure = { ...status, needs_setup: true, phase: 'error', environment_step: 'pulling',
    message: { code: 'system-docker-command-failed', args: { status: 'exit code: 1', log: 'C:\\cache\\pull.log' }, detail: 'error getting credentials: helper missing' } };
  vi.mocked(api<'update_action'>).mockImplementation(async (_name, args) => args?.action === 'setup' ? { ...status, needs_setup: false, available: null } : failure);
  render(<Updates epoch={1} onClose={() => {}} onChanged={() => {}} />);
  await screen.findByRole('alert');
  expect(screen.getAllByText(/Docker 执行失败/)).toHaveLength(1);
  expect(screen.getByRole('button', { name: '复制错误' })).toBeTruthy();
  await userEvent.click(screen.getByText('查看诊断输出（日志末尾）'));
  expect(screen.getByText('error getting credentials: helper missing')).toBeTruthy();
  await userEvent.click(screen.getByRole('button', { name: '重试准备' }));
  await screen.findByRole('button', { name: '检查更新' });
  expect(screen.queryByRole('alert')).toBeNull();
});

test('reopening during preparation retains actual progress and blocks competing operations', async () => {
  vi.mocked(api<'update_action'>).mockResolvedValue({ ...status, needs_setup: true, phase: 'pulling', environment_step: 'pulling' });
  render(<Updates epoch={1} onClose={() => {}} onChanged={() => {}} />);
  const list = await screen.findByRole('list', { name: '环境准备步骤' });
  expect(list.children[0].getAttribute('data-state')).toBe('done');
  expect(list.children[1].getAttribute('aria-current')).toBe('step');
  expect(screen.getByRole('button', { name: '准备工具环境' }).closest('fieldset')?.disabled).toBe(true);
});

test('tools repair uses the shared update operation rather than a raw workbench result', async () => {
  vi.mocked(api<'update_action'>).mockResolvedValue(status);
  render(<Updates mode="tools" epoch={1} onClose={() => {}} onChanged={() => {}} />);
  await screen.findByText('0.2.0');
  await userEvent.click(screen.getByRole('button', { name: '自检并修复环境' }));
  expect(vi.mocked(api<'update_action'>).mock.calls.some(([name, args]) => name === 'update_action' && args?.action === 'setup')).toBe(true);
});

test('a running preparation can be hidden and cancellation targets its operation id', async () => {
  const running = { ...status, busy: true, operation_id: 'prepare-123', cancellable: true, phase: 'pulling' };
  vi.mocked(api<'update_action'>).mockResolvedValue(running);
  const close = vi.fn();
  render(<Updates epoch={1} onClose={close} onChanged={() => {}} />);
  await screen.findByText('0.2.0');
  await userEvent.click(screen.getByRole('button', { name: '收起到后台' }));
  expect(close).toHaveBeenCalledOnce();
  await userEvent.click(screen.getByRole('button', { name: '取消准备' }));
  expect(api).toHaveBeenCalledWith('update_cancel', { operationId: 'prepare-123' });
});

test('activation explains why cancellation is unavailable', async () => {
  vi.mocked(api<'update_action'>).mockResolvedValue({ ...status, busy: true, phase: 'activating', cancellable: false });
  render(<Updates epoch={1} onClose={() => {}} onChanged={() => {}} />);
  await screen.findByText('0.2.0');
  expect((screen.getByRole('button', { name: '取消准备' }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByText(/当前阶段不支持取消/)).toBeTruthy();
});
