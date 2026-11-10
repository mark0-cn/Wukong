import { useEffect, useRef, useState } from 'react';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { api, errorText, subscribe } from '../bridge';

interface Lifecycle {
  busy: boolean; hasDraft: boolean;
  onConfirm: (busy: boolean) => void;
  onTarget: (path?: string) => void;
  onError: (message: string) => void;
}

export function useDesktopLifecycle(options: Lifecycle) {
  const current = useRef(options); current.current = options;
  const [closing, setClosing] = useState(false);
  const close = async () => {
    setClosing(true); current.current.onError('');
    try { await api('shutdown_workspace'); }
    catch (error) { current.current.onError(errorText(error)); setClosing(false); }
  };
  useEffect(() => {
    let alive = true; const disposers: (() => void)[] = [];
    const register = (promise: Promise<() => void>) => {
      void promise.then(dispose => { if (alive) disposers.push(dispose); else dispose(); }).catch(() => {});
    };
    register(subscribe('close-requested', event => {
      const state = current.current;
      if (state.busy || state.hasDraft || event.payload.busy) state.onConfirm(state.busy || event.payload.busy);
      else void close();
    }));
    try {
      register(getCurrentWebview().onDragDropEvent(event => {
        if (event.payload.type === 'drop' && event.payload.paths[0]) current.current.onTarget(event.payload.paths[0]);
      }));
    } catch { /* Browser previews have no native drop surface. */ }
    const keyboard = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'n') {
        event.preventDefault(); current.current.onTarget();
      }
    };
    window.addEventListener('keydown', keyboard);
    return () => { alive = false; disposers.forEach(dispose => dispose()); window.removeEventListener('keydown', keyboard); };
  }, []);
  return { close, closing };
}
