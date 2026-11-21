import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, vi, describe, it, expect } from 'vitest';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { api, subscribe } from '../../shared/api/bridge';
import { useWorkspace } from './model';
import { envelope, workspace } from '../../shared/testing/fixtures';
import type { Envelope } from '../../shared/api/types';

vi.mock('../../shared/api/bridge', () => ({ api: vi.fn(), errorText: String, errorValue: String, subscribe: vi.fn() }));

function renderWorkspace() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, networkMode: 'always', refetchOnWindowFocus: false, refetchOnReconnect: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  return renderHook(useWorkspace, { wrapper });
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
function lastRequest() {
  return vi.mocked(api<'workspace_read'>).mock.calls.at(-1)?.[1]?.request as {
    run?: string | null; agent?: string | null; event_limit?: number; known_revision?: string | null;
  };
}

beforeEach(() => {
  vi.mocked(api).mockReset();
  vi.mocked(subscribe).mockReset();
  vi.mocked(subscribe).mockResolvedValue(() => {});
});

describe('workspace isolation', () => {
  it('ignores an old response after the user selects another run', async () => {
    const old = deferred<Envelope>();
    vi.mocked(api<'workspace_read'>).mockImplementation((_command, args) => {
      if (args?.request.run === 'run-b') return Promise.resolve(envelope({ ...workspace, selected: 'run-b', transcript: [] }));
      return old.promise;
    });
    const hook = renderWorkspace();
    await waitFor(() => expect(api).toHaveBeenCalledTimes(1));
    await act(async () => { hook.result.current.selectRun('run-b'); });
    await waitFor(() => expect(hook.result.current.data?.selected).toBe('run-b'));
    await act(async () => { old.resolve(envelope()); });
    expect(hook.result.current.data?.selected).toBe('run-b');
  });

  it('retains the current snapshot when the server returns only a revision', async () => {
    vi.mocked(api<'workspace_read'>).mockResolvedValue(envelope());
    const hook = renderWorkspace();
    await waitFor(() => expect(hook.result.current.data?.selected).toBe('run-a'));
    await waitFor(() => expect(hook.result.current.loading).toBe(false));
    vi.mocked(api<'workspace_read'>).mockResolvedValue({ host: envelope().host, workspace: { revision: 'revision-a', data: null } });
    await act(async () => { hook.result.current.refresh(); });
    expect(hook.result.current.data?.transcript).toHaveLength(4);
    expect(lastRequest().known_revision).toBe('revision-a');
  });

  it('shows no previous-run placeholder and retains host metadata while the new key loads', async () => {
    vi.mocked(api<'workspace_read'>).mockResolvedValue(envelope());
    const hook = renderWorkspace();
    await waitFor(() => expect(hook.result.current.loading).toBe(false));
    const next = deferred<Envelope>();
    vi.mocked(api<'workspace_read'>).mockImplementation(() => next.promise);
    const originalHost = hook.result.current.host;
    await act(async () => { hook.result.current.selectRun('run-b'); });
    expect(hook.result.current.data).toBeUndefined();
    expect(hook.result.current.host).toBe(originalHost);
    await act(async () => { next.resolve(envelope({ ...workspace, selected: 'run-b', transcript: [] })); });
    await waitFor(() => expect(hook.result.current.data?.selected).toBe('run-b'));
  });

  it('recovers from a read error only through an explicit refresh', async () => {
    vi.mocked(api<'workspace_read'>).mockRejectedValueOnce(new Error('workspace unavailable'));
    vi.mocked(api<'workspace_read'>).mockResolvedValue(envelope({ ...workspace, selected: null }));
    const hook = renderWorkspace();
    await waitFor(() => expect(hook.result.current.error).toContain('workspace unavailable'));
    expect(hook.result.current.loading).toBe(false);
    expect(api).toHaveBeenCalledTimes(1);
    await act(async () => { hook.result.current.refresh(); });
    await waitFor(() => expect(hook.result.current.error).toBe(''));
    expect(hook.result.current.data?.selected).toBeNull();
    expect(api).toHaveBeenCalledTimes(2);
  });
});

describe('workspace reconnection', () => {
  it('attaches events before reading the initial snapshot', async () => {
    let finish!: (dispose: () => void) => void;
    vi.mocked(subscribe).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    vi.mocked(api<'workspace_read'>).mockResolvedValue(envelope());
    const hook = renderWorkspace();
    expect(api).not.toHaveBeenCalled();
    await act(async () => { finish(() => {}); });
    await waitFor(() => expect(hook.result.current.data?.selected).toBe('run-a'));
    hook.unmount();
  });

  it('uses a snapshot after failed subscription and reattaches on focus', async () => {
    vi.mocked(subscribe).mockRejectedValueOnce(new Error('disconnected'));
    vi.mocked(api<'workspace_read'>).mockResolvedValue(envelope({ ...workspace, selected: null }));
    const hook = renderWorkspace();
    await waitFor(() => expect(hook.result.current.loading).toBe(false));
    const before = vi.mocked(subscribe).mock.calls.length;
    await act(async () => { window.dispatchEvent(new Event('focus')); });
    expect(vi.mocked(subscribe).mock.calls.length).toBe(before + 1);
    expect(hook.result.current.error).toBe('');
    hook.unmount();
  });

  it('coalesces active refreshes into exactly one trailing read', async () => {
    const first = deferred<Envelope>();
    const trailing = deferred<Envelope>();
    vi.mocked(api<'workspace_read'>).mockImplementationOnce(() => first.promise).mockImplementationOnce(() => trailing.promise);
    const hook = renderWorkspace();
    await waitFor(() => expect(api).toHaveBeenCalledTimes(1));
    await act(async () => { hook.result.current.refresh(); hook.result.current.refresh(); });
    expect(api).toHaveBeenCalledTimes(1);
    await act(async () => { first.resolve(envelope({ ...workspace, selected: null })); });
    await waitFor(() => expect(api).toHaveBeenCalledTimes(2));
    await act(async () => { trailing.resolve({ host: envelope().host, workspace: { revision: 'revision-next', data: null } }); });
    await waitFor(() => expect(hook.result.current.loading).toBe(false));
    expect(api).toHaveBeenCalledTimes(2);
    expect(hook.result.current.data?.selected).toBeNull();
  });

  it('polls, recovers on visibility and stops all recovery after unmount', async () => {
    vi.useFakeTimers();
    try {
      vi.mocked(api<'workspace_read'>).mockResolvedValue(envelope({ ...workspace, selected: null }));
      const hook = renderWorkspace();
      await act(async () => { await vi.advanceTimersByTimeAsync(0); });
      expect(hook.result.current.loading).toBe(false);
      expect(api).toHaveBeenCalledTimes(1);
      await act(async () => { await vi.advanceTimersByTimeAsync(3000); });
      expect(api).toHaveBeenCalledTimes(2);
      await act(async () => {
        document.dispatchEvent(new Event('visibilitychange'));
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(api).toHaveBeenCalledTimes(3);
      hook.unmount();
      await act(async () => { window.dispatchEvent(new Event('focus')); });
      expect(api).toHaveBeenCalledTimes(3);
    } finally {
      vi.useRealTimers();
    }
  });

  it('keys history and reset generations without reusing a known revision', async () => {
    vi.mocked(api<'workspace_read'>).mockResolvedValue(envelope({ ...workspace, selected: null }));
    const hook = renderWorkspace();
    await waitFor(() => expect(hook.result.current.loading).toBe(false));
    await act(async () => { hook.result.current.loadEarlier(); });
    await waitFor(() => expect(lastRequest().event_limit).toBe(1000));
    await act(async () => { hook.result.current.selectAgent('agent'); });
    await waitFor(() => expect(lastRequest().agent).toBe('agent'));
    expect(lastRequest().event_limit).toBe(500);
    await act(async () => { hook.result.current.reset(); });
    await waitFor(() => expect(lastRequest().known_revision).toBeNull());
    expect(lastRequest().agent).toBeNull();
    expect(hook.result.current.historyLimit).toBe(500);
  });
});
