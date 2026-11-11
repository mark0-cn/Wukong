import { useSyncExternalStore, type SetStateAction } from 'react';

const memory = new Map<string, string>();
const event = 'wukong-view-preferences';
function read(key: string) {
  try { return memory.get(key) ?? localStorage.getItem(key); }
  catch { return memory.get(key) ?? null; }
}
function subscribe(notify: () => void) {
  window.addEventListener(event, notify);
  window.addEventListener('storage', notify);
  return () => { window.removeEventListener(event, notify); window.removeEventListener('storage', notify); };
}
/** Device-local presentation preferences, never task execution state. */
export function useViewPreference<T>(name: string, fallback: T, valid: (value: unknown) => value is T) {
  const key = `wukong-view:${name}`;
  const raw = useSyncExternalStore(subscribe, () => read(key));
  const parse = (source: string | null): T => {
    try { const value: unknown = source === null ? undefined : JSON.parse(source); return valid(value) ? value : fallback; }
    catch { return fallback; }
  };
  const set = (action: SetStateAction<T>) => {
    const next = typeof action === 'function' ? (action as (previous: T) => T)(parse(read(key))) : action;
    const source = JSON.stringify(next);
    try { localStorage.setItem(key, source); memory.delete(key); }
    catch { memory.set(key, source); }
    window.dispatchEvent(new Event(event));
  };
  return [parse(raw), set] as const;
}
export const booleanPreference = (value: unknown): value is boolean => typeof value === 'boolean';
export const optionalBooleanPreference = (value: unknown): value is boolean | null => value === null || typeof value === 'boolean';
export const runNamesPreference = (value: unknown): value is Record<string, string> => Boolean(value && typeof value === 'object' && !Array.isArray(value) && Object.values(value).every(name => typeof name === 'string' && name.length <= 100));
