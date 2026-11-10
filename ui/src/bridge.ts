import { t, message, type LocalizedMessage } from './i18n';
import { invoke } from '@tauri-apps/api/core';
import { listen, type EventCallback } from '@tauri-apps/api/event';
import { openUrl } from '@tauri-apps/plugin-opener';
import type { Commands, Events } from './commands';

export const api = <K extends keyof Commands>(command: K, ...args: Commands[K]['args'] extends undefined
  ? [args?: undefined] : [args: Commands[K]['args']]): Promise<Commands[K]['result']> =>
  invoke<Commands[K]['result']>(command, args[0]);
export const subscribe = <K extends keyof Events>(event: K, callback: EventCallback<Events[K]>) => listen<Events[K]>(event, callback);
export const openLink = (url: string) => {
  if (/^https?:\/\//i.test(url)) return openUrl(url);
  return Promise.reject(new Error(t('workspace-only-http-and-https-links-can-be')));
};
export const errorValue = (error: unknown): string | LocalizedMessage => error && typeof error === 'object' && 'code' in error && 'args' in error
  ? error as LocalizedMessage : error instanceof Error ? error.message : String(error ?? '');
export const errorText = (error: unknown) => message(errorValue(error));
