import { useSyncExternalStore, type ReactNode } from 'react';
import { FluentBundle, FluentResource, type FluentVariable } from '@fluent/bundle';
import { LocalizationProvider, ReactLocalization } from '@fluent/react';
import { invoke } from '@tauri-apps/api/core';
import type { Messages } from './messages';

export type Locale = 'en-US' | 'zh-CN';
export type LanguageChoice = Locale | 'auto';
export interface Preferences { ui_language: LanguageChoice; output_language: LanguageChoice }
export interface PreferenceSnapshot extends Preferences { system_locale: Locale }
export interface LocalizedMessage { code: string; args: Record<string, string | number>; detail?: string }
const resources = import.meta.glob('../../../locales/*/*.ftl', { query: '?raw', import: 'default', eager: true }) as Record<string, string>;
const bundles = Object.fromEntries((['en-US', 'zh-CN'] as const).map(locale => {
  const bundle = new FluentBundle(locale, { useIsolating: false });
  for (const [path, source] of Object.entries(resources)) if (path.includes(`/${locale}/`)) {
    const errors = bundle.addResource(new FluentResource(source));
    if (errors.length) throw errors[0];
  }
  return [locale, bundle];
})) as Record<Locale, FluentBundle>;
export function negotiate(tag: string): Locale { return /^zh(?:$|[-_](?:CN|Hans|SG)(?:[-_]|$))/i.test(tag) ? 'zh-CN' : 'en-US'; }
let snapshot: PreferenceSnapshot = { ui_language: 'auto', output_language: 'auto', system_locale: negotiate(navigator.language) };
const listeners = new Set<() => void>();
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
export const preferences = () => snapshot;
export const currentLocale = (): Locale => snapshot.ui_language === 'auto' ? snapshot.system_locale : snapshot.ui_language;
function publish(next: PreferenceSnapshot) {
  snapshot = next;
  document.documentElement.lang = currentLocale();
  document.title = t('app-title');
  listeners.forEach(listener => listener());
}
export async function initializeLanguage() {
  try { publish(await invoke<PreferenceSnapshot>('preferences_read')); }
  catch (error) { console.error('Language preferences unavailable', error); publish(snapshot); }
}
export async function saveLanguage(next: Preferences) {
  publish(await invoke<PreferenceSnapshot>('preferences_save', { preferences: next }));
}
export function usePreferences() { return useSyncExternalStore(subscribe, preferences); }
export function useLocale() { usePreferences(); return currentLocale(); }
const localizations = { 'en-US': new ReactLocalization([bundles['en-US']]), 'zh-CN': new ReactLocalization([bundles['zh-CN'], bundles['en-US']]) };
export function LanguageProvider({ children }: { children: ReactNode }) {
  const locale = useLocale();
  return <LocalizationProvider l10n={localizations[locale]}>{children}</LocalizationProvider>;
}
export function text(code: string, args?: Record<string, FluentVariable>, fallback?: string): string {
  for (const locale of [currentLocale(), 'en-US'] as const) {
    const bundle = bundles[locale], message = bundle.getMessage(code);
    if (message?.value) return bundle.formatPattern(message.value, args);
  }
  return fallback ?? code;
}
export function t<K extends keyof Messages>(code: K, ...args: Messages[K] extends undefined ? [] : [Messages[K]]): string {
  return text(code, args[0]);
}
export function message(value: LocalizedMessage | string): string {
  if (typeof value === 'string') return value; // Verbatim legacy diagnostics.
  if (!value.code) return value.detail ?? '';
  const rendered = text(value.code, value.args, t('error-operation'));
  return value.detail ? `${rendered}\n${value.detail}` : rendered;
}
