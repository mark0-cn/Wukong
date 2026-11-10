import { useEffect, useRef } from 'react';
import { api } from '../bridge';
import type { Host } from '../types';

export function useStartupUpdates(host: Host | undefined, onOpen: () => void) {
  const checked = useRef(false);
  useEffect(() => {
    if (!host || checked.current) return;
    checked.current = true;
    void api('update_action', { action: 'status', epoch: host.epoch }).then(status => {
      if (status.needs_setup || status.pending) onOpen();
      else void api('update_action', { action: 'check', epoch: host.epoch }).catch(() => {});
    }).catch(() => {});
  }, [host, onOpen]);
}
