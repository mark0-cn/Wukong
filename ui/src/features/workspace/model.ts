import { useCallback, useEffect, useRef, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { api, errorValue, subscribe } from '../../shared/api/bridge';
import type { Host, Workspace } from '../../shared/api/types';

interface WorkspaceRead {
  host: Host;
  revision: string;
  snapshot: Workspace | undefined;
}

export function useWorkspace() {
  const queryClient = useQueryClient();
  const [run, setRun] = useState<string>();
  const [agent, setAgent] = useState<string>();
  const [limit, setLimit] = useState(500);
  const [resetGeneration, setResetGeneration] = useState(0);
  const reload = useRef<() => void>(() => {});
  const lastHost = useRef<Host | undefined>(undefined);
  const refresh = useCallback(() => reload.current(), []);
  const reset = useCallback(() => {
    setRun(undefined); setAgent(undefined); setLimit(500);
    setResetGeneration(value => value + 1);
  }, []);
  const selectRun = useCallback((id: string) => {
    setRun(id); setAgent(undefined); setLimit(500);
  }, []);
  const selectAgent = useCallback((id?: string) => { setAgent(id); setLimit(500); }, []);
  const queryKey = ['workspace', run ?? null, agent ?? null, limit, resetGeneration] as const;
  const result = useQuery({
    queryKey,
    queryFn: async (): Promise<WorkspaceRead> => {
      const previous = queryClient.getQueryData<WorkspaceRead>(queryKey);
      const response = await api('workspace_read', { request: {
        run: run ?? null, agent: agent ?? null, event_limit: limit, known_revision: previous?.revision ?? null,
      } });
      return {
        host: response.host,
        revision: response.workspace.revision,
        snapshot: response.workspace.data ?? previous?.snapshot,
      };
    },
    enabled: false,
    retry: false,
    gcTime: 0,
    networkMode: 'always',
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  });
  const responseHost = result.data?.host;
  const snapshot = result.data?.snapshot;
  useEffect(() => { if (responseHost) lastHost.current = responseHost; }, [responseHost]);
  useEffect(() => {
    if (!run && snapshot?.selected) setRun(snapshot.selected);
  }, [run, snapshot]);
  const refetch = result.refetch;
  useEffect(() => {
    let alive = true, inFlight = false, requested = false;
    let unlisten: (() => void) | undefined;
    let subscribing = false;
    const load = async () => {
      if (!alive) return;
      if (inFlight) { requested = true; return; }
      inFlight = true;
      try { await refetch({ cancelRefetch: false }); }
      finally {
        inFlight = false;
        if (alive && requested) { requested = false; void load(); }
      }
    };
    reload.current = () => { void load(); };
    const attach = async () => {
      if (!alive || unlisten || subscribing) return;
      subscribing = true;
      try {
        const dispose = await subscribe('workspace-changed', () => { void load(); });
        if (alive) unlisten = dispose; else dispose();
      } catch { /* Polling remains available while the event bridge reconnects. */ }
      finally { subscribing = false; if (alive) void load(); }
    };
    void attach();
    const catchUp = () => { void attach(); void load(); };
    const visible = () => { if (document.visibilityState === 'visible') catchUp(); };
    window.addEventListener('focus', catchUp);
    document.addEventListener('visibilitychange', visible);
    const interval = window.setInterval(catchUp, 3000);
    return () => {
      alive = false; window.clearInterval(interval); unlisten?.();
      window.removeEventListener('focus', catchUp);
      document.removeEventListener('visibilitychange', visible);
    };
  }, [run, agent, limit, resetGeneration, refetch]);
  return { run, agent, host: responseHost ?? lastHost.current, data: snapshot,
    error: result.error ? errorValue(result.error) : '',
    loading: result.isFetching || (!result.data && !result.error), refresh, reset, selectRun, selectAgent,
    loadEarlier: () => setLimit(value => Math.min(10_000, value + 500)), historyLimit: limit };
}
export type WorkspaceState = ReturnType<typeof useWorkspace>;
