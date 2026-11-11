import { useCallback, useEffect, useRef, useState } from 'react';
import { api, errorValue } from '../bridge';
import type { UpdateAction, UpdateStatus } from '../commands';

export function newerUpdate(previous: UpdateStatus | undefined, next: UpdateStatus): boolean {
  return previous?.revision === undefined || next.revision === undefined || next.revision >= previous.revision;
}

/** One subscription can outlive every operation panel. Polls never overlap. */
export function useUpdateController(epoch: number | undefined, onChanged: () => void, enabled = true) {
  const [status, setStatus] = useState<UpdateStatus>();
  const [pending, setPending] = useState(false);
  const [operationError, setOperationError] = useState<ReturnType<typeof errorValue>>('');
  const [refreshError, setRefreshError] = useState<ReturnType<typeof errorValue>>('');
  const accepted = useRef<UpdateStatus | undefined>(undefined);
  const sequence = useRef(0), acceptedSequence = useRef(0), actionBoundary = useRef(0);
  const currentEpoch = useRef(epoch); currentEpoch.current = epoch;
  const alive = useRef(true);
  const changed = useRef(onChanged); changed.current = onChanged;
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const accept = useCallback((value: UpdateStatus, request: number) => {
    if (request < actionBoundary.current || !newerUpdate(accepted.current, value)) return;
    if (value.revision === undefined && request < acceptedSequence.current) return;
    const prior = accepted.current;
    accepted.current = value; acceptedSequence.current = request; setStatus(value);
    if (prior?.operation_id && prior.operation_id !== value.operation_id) setOperationError('');
  }, []);
  useEffect(() => {
    if (!enabled || epoch === undefined) return;
    let active = true; let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      const request = ++sequence.current;
      try {
        const value = await api('update_action', { action: 'status', epoch });
        if (active && request >= actionBoundary.current) { accept(value, request); setRefreshError(''); }
      } catch (error) {
        if (active && request >= actionBoundary.current) setRefreshError(errorValue(error));
      } finally { if (active) timer = setTimeout(() => { void poll(); }, 1000); }
    };
    void poll();
    return () => { active = false; clearTimeout(timer); };
  }, [epoch, enabled, accept]);
  const run = useCallback(async (action: UpdateAction) => {
    if (epoch === undefined) return false;
    const request = ++sequence.current; actionBoundary.current = request;
    setPending(true); setOperationError('');
    try {
      const value = await api('update_action', { action, epoch });
      if (alive.current && epoch === currentEpoch.current) { accept(value, request); changed.current(); }
      return true;
    } catch (error) {
      if (alive.current && epoch === currentEpoch.current) setOperationError(errorValue(error));
      return false;
    } finally { if (alive.current) setPending(false); }
  }, [epoch, accept]);
  const working = pending || (status?.busy ?? ['starting', 'checking', 'downloading', 'environment', 'docker', 'pulling', 'validating', 'activating', 'installing', 'storage'].includes(status?.phase ?? ''));
  return { status, working, operationError, refreshError, run };
}
export type UpdateController = ReturnType<typeof useUpdateController>;
