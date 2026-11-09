import { useCallback, useEffect, useRef, useState } from 'react';
import { api, errorText, subscribe } from './bridge';
import type { Envelope, Host, Workspace } from './types';

export function useWorkspace() {
  const [run, setRun] = useState<string>();
  const [agent, setAgent] = useState<string>();
  const [limit, setLimit] = useState(500);
  const [refreshKey, setRefreshKey] = useState(0);
  const [host, setHost] = useState<Host>();
  const [data, setData] = useState<Workspace>();
  const [error, setError] = useState('');
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
    const selection = `${run ?? ''}|${agent ?? ''}|${refreshKey}`;
    if (previousSelection.current !== selection) setData(undefined);
    previousSelection.current = selection;
    setLoading(true); setError('');
    const load = async () => {
      if (!alive) return;
      if (inFlight) { requested = true; return; }
      inFlight = true;
      try {
        const response = await api<Envelope>('workspace_read', { request: {
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
      } catch (error) { if (alive) setError(errorText(error)); }
      finally {
        inFlight = false;
        if (alive) {
          setLoading(false);
          if (requested) { requested = false; void load(); }
        }
      }
    };
    reload.current = () => { void load(); };
    void load();
    void subscribe('workspace-changed', () => { void load(); }).then(dispose => {
      if (alive) unlisten = dispose; else dispose();
    }).catch(() => {});
    const interval = window.setInterval(() => { void load(); }, 3000);
    return () => { alive = false; window.clearInterval(interval); unlisten?.(); };
  }, [run, agent, limit, refreshKey]);
  return { run, agent, host, data, error, loading, refresh, reset, selectRun, selectAgent,
    loadEarlier: () => setLimit(value => Math.min(10_000, value + 500)), historyLimit: limit };
}
export type WorkspaceState = ReturnType<typeof useWorkspace>;
