import { useCallback, useEffect, useRef, useState } from 'react';
import { api, errorValue, subscribe } from '../../shared/api/bridge';
import type { Envelope, Host, Workspace } from '../../shared/api/types';

export function useWorkspace() {
  const [run, setRun] = useState<string>();
  const [agent, setAgent] = useState<string>();
  const [limit, setLimit] = useState(500);
  const [refreshKey, setRefreshKey] = useState(0);
  const [host, setHost] = useState<Host>();
  const [data, setData] = useState<Workspace>();
  const [error, setError] = useState<ReturnType<typeof errorValue>>('');
  const [loading, setLoading] = useState(true);
  const reload = useRef<() => void>(() => {});
  const previousSelection = useRef('');
  const refresh = useCallback(() => reload.current(), []);
  const reset = useCallback(() => {
    setRun(undefined); setAgent(undefined); setLimit(500); setData(undefined);
    setRefreshKey(value => value + 1);
  }, []);
  const selectRun = useCallback((id: string) => {
    setRun(id); setAgent(undefined); setLimit(500);
  }, []);
  const selectAgent = useCallback((id?: string) => { setAgent(id); setLimit(500); }, []);
  useEffect(() => {
    let alive = true, inFlight = false, requested = false, revision: string | undefined;
    let unlisten: (() => void) | undefined;
    let subscribing = false;
    const selection = `${run ?? ''}|${agent ?? ''}|${refreshKey}`;
    if (previousSelection.current !== selection) setData(undefined);
    previousSelection.current = selection;
    setLoading(true); setError('');
    const load = async () => {
      if (!alive) return;
      if (inFlight) { requested = true; return; }
      inFlight = true;
      try {
        const response = await api('workspace_read', { request: {
          run: run ?? null, agent: agent ?? null, event_limit: limit, known_revision: revision ?? null,
        } });
        if (!alive) return;
        setHost(response.host);
        if (response.workspace.data) {
          setData(response.workspace.data);
          if (!run && response.workspace.data.selected) setRun(response.workspace.data.selected);
        }
        revision = response.workspace.revision;
        setError('');
      } catch (error) { if (alive) setError(errorValue(error)); }
      finally {
        inFlight = false;
        if (alive) {
          setLoading(false);
          if (requested) { requested = false; void load(); }
        }
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
  }, [run, agent, limit, refreshKey]);
  return { run, agent, host, data, error, loading, refresh, reset, selectRun, selectAgent,
    loadEarlier: () => setLimit(value => Math.min(10_000, value + 500)), historyLimit: limit };
}
export type WorkspaceState = ReturnType<typeof useWorkspace>;
