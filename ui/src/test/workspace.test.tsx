import { act, renderHook, waitFor } from '@testing-library/react';
import { vi, describe, it, expect } from 'vitest';
import { api } from '../bridge';
import { useWorkspace } from '../workspace';
import { envelope, workspace } from './fixtures';
vi.mock('../bridge', () => ({ api: vi.fn(), errorText: String, subscribe: vi.fn().mockResolvedValue(() => {}) }));

describe('workspace isolation', () => {
  it('ignores an old response after the user selects another run', async () => {
    let resolveOld!: (value: ReturnType<typeof envelope>) => void;
    vi.mocked(api).mockReset().mockImplementation((_command, args) => {
      if ((args?.request as { run?: string })?.run === 'run-b') return Promise.resolve(envelope({ ...workspace, selected: 'run-b', transcript: [] }));
      return new Promise(resolve => { resolveOld = resolve as typeof resolveOld; });
    });
    const hook = renderHook(useWorkspace);
    await act(async () => { hook.result.current.selectRun('run-b'); });
    await waitFor(() => expect(hook.result.current.data?.selected).toBe('run-b'));
    await act(async () => { resolveOld(envelope()); });
    expect(hook.result.current.data?.selected).toBe('run-b');
  });
  it('retains the current snapshot when the server returns only a revision', async () => {
    vi.mocked(api).mockReset().mockResolvedValue(envelope());
    const hook = renderHook(useWorkspace);
    await waitFor(() => expect(hook.result.current.data?.selected).toBe('run-a'));
    await waitFor(() => expect(hook.result.current.loading).toBe(false));
    vi.mocked(api).mockResolvedValue({ host: envelope().host, workspace: { revision: 'revision-a', data: null } });
    await act(async () => { hook.result.current.refresh(); });
    expect(hook.result.current.data?.transcript).toHaveLength(4);
    const lastArgs = vi.mocked(api).mock.calls.at(-1)?.[1]?.request as { known_revision?: string };
    expect(lastArgs.known_revision).toBe('revision-a');
  });
});
