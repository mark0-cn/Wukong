import { act, renderHook, waitFor } from '@testing-library/react';
import { vi, describe, it, expect } from 'vitest';
import { api, subscribe } from '../../shared/api/bridge';
import { useWorkspace } from './model';
import { envelope, workspace } from '../../shared/testing/fixtures';
vi.mock('../../shared/api/bridge', () => ({ api: vi.fn(), errorText: String, errorValue: String, subscribe: vi.fn().mockResolvedValue(() => {}) }));

describe('workspace isolation', () => {
  it('ignores an old response after the user selects another run', async () => {
    let resolveOld!: (value: ReturnType<typeof envelope>) => void;
    vi.mocked(api<'workspace_read'>).mockReset().mockImplementation((_command, args) => {
      if ((args?.request as { run?: string })?.run === 'run-b') return Promise.resolve(envelope({ ...workspace, selected: 'run-b', transcript: [] }));
      return new Promise(resolve => { resolveOld = resolve as typeof resolveOld; });
    });
    const hook = renderHook(useWorkspace);
    await waitFor(() => expect(resolveOld).toBeTypeOf('function'));
    await act(async () => { hook.result.current.selectRun('run-b'); });
    await waitFor(() => expect(hook.result.current.data?.selected).toBe('run-b'));
    await act(async () => { resolveOld(envelope()); });
    expect(hook.result.current.data?.selected).toBe('run-b');
  });
  it('retains the current snapshot when the server returns only a revision', async () => {
    vi.mocked(api<'workspace_read'>).mockReset().mockResolvedValue(envelope());
    const hook = renderHook(useWorkspace);
    await waitFor(() => expect(hook.result.current.data?.selected).toBe('run-a'));
    await waitFor(() => expect(hook.result.current.loading).toBe(false));
    vi.mocked(api<'workspace_read'>).mockResolvedValue({ host: envelope().host, workspace: { revision: 'revision-a', data: null } });
    await act(async () => { hook.result.current.refresh(); });
    expect(hook.result.current.data?.transcript).toHaveLength(4);
    const lastArgs = vi.mocked(api<'workspace_read'>).mock.calls.at(-1)?.[1]?.request as { known_revision?: string };
    expect(lastArgs.known_revision).toBe('revision-a');
  });
});


describe('workspace reconnection', () => {
  it('attaches events before reading the initial snapshot', async () => {
    let finish!: (dispose: () => void) => void;
    vi.mocked(subscribe).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    vi.mocked(api<'workspace_read'>).mockReset().mockResolvedValue(envelope());
    const hook = renderHook(useWorkspace);
    expect(api).not.toHaveBeenCalled();
    await act(async () => { finish(() => {}); });
    await waitFor(() => expect(hook.result.current.data?.selected).toBe('run-a'));
    hook.unmount();
  });
  it('uses a snapshot after failed subscription and reattaches on focus', async () => {
    vi.mocked(subscribe).mockRejectedValueOnce(new Error('disconnected'));
    vi.mocked(api<'workspace_read'>).mockReset().mockResolvedValue(envelope({ ...workspace, selected: null }));
    const hook = renderHook(useWorkspace);
    await waitFor(() => expect(hook.result.current.loading).toBe(false));
    const before = vi.mocked(subscribe).mock.calls.length;
    await act(async () => { window.dispatchEvent(new Event('focus')); });
    expect(vi.mocked(subscribe).mock.calls.length).toBe(before + 1);
    expect(hook.result.current.error).toBe('');
    hook.unmount();
  });
});
