import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { api } from '../bridge';
import type { UpdateStatus } from '../commands';
import { useUpdateController } from './useUpdateController';
vi.mock('../bridge', async () => ({ ...await vi.importActual('../bridge'), api: vi.fn() }));
const state = { revision: 5, operation_id: 'operation-b', busy: false, phase: 'idle', version: '0.2.0', message: '' } as UpdateStatus;
afterEach(() => { vi.useRealTimers(); vi.mocked(api).mockReset(); });

test('polls do not overlap and an old response cannot replace a newer operation', async () => {
  vi.useFakeTimers();
  let resolve!: (value: UpdateStatus) => void;
  vi.mocked(api<'update_action'>).mockImplementation(async (_name, args) => args?.action === 'status' ? new Promise(r => { resolve = r; }) : state);
  const { result } = renderHook(() => useUpdateController(1, () => {}));
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(vi.mocked(api).mock.calls).toHaveLength(1);
  await act(async () => { await result.current.run('setup'); });
  await act(async () => { resolve({ ...state, revision: 1, operation_id: 'old', phase: 'error' }); });
  expect(result.current.status?.operation_id).toBe('operation-b');
  expect(result.current.status?.phase).toBe('idle');
});

test('a recovered refresh clears only the connection error and retains operation errors', async () => {
  vi.useFakeTimers();
  vi.mocked(api<'update_action'>).mockRejectedValueOnce(new Error('temporary IPC failure')).mockResolvedValue({ ...state, phase: 'error', message: 'Docker failed' });
  const { result } = renderHook(() => useUpdateController(1, () => {}));
  await act(async () => {});
  expect(result.current.refreshError).toBe('temporary IPC failure');
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(result.current.refreshError).toBe('');
  expect(result.current.status?.message).toBe('Docker failed');
});
