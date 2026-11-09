import { invoke } from '@tauri-apps/api/core';
import { listen, type EventCallback } from '@tauri-apps/api/event';
import { openUrl } from '@tauri-apps/plugin-opener';

export const api = <T,>(command: string, args: Record<string, unknown> = {}) => invoke<T>(command, args);
export const subscribe = <T,>(event: string, callback: EventCallback<T>) => listen<T>(event, callback);
export const openLink = (url: string) => {
  if (/^https?:\/\//i.test(url)) return openUrl(url);
  return Promise.reject(new Error('仅支持打开 HTTP 或 HTTPS 链接。'));
};
export const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);
