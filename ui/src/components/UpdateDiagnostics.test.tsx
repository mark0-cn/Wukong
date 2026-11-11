import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';
import { api } from '../bridge';
import type { UpdateStatus } from '../commands';
import { UpdateDiagnostics } from './UpdateDiagnostics';
vi.mock('../bridge', async () => ({ ...await vi.importActual('../bridge'), api: vi.fn() }));

test('diagnostic actions use an operation id and never send a log path from the UI', async () => {
  vi.mocked(api).mockResolvedValue(null);
  const retry = vi.fn();
  render(<UpdateDiagnostics status={{ operation_id: 'operation-a', diagnosis: 'credentials', log_available: true } as UpdateStatus} retry={retry} />);
  expect(screen.getByText(/凭据助手或登录状态异常/)).toBeTruthy();
  await userEvent.click(screen.getByRole('button', { name: '打开日志目录' }));
  expect(api).toHaveBeenCalledWith('update_log_directory', { operationId: 'operation-a' });
  await userEvent.click(screen.getByRole('button', { name: '导出脱敏诊断包' }));
  expect(api).toHaveBeenCalledWith('update_diagnostics', { operationId: 'operation-a' });
  await userEvent.click(screen.getByRole('button', { name: '处理后重试' }));
  expect(retry).toHaveBeenCalledOnce();
});
