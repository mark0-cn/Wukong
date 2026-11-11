import { useEffect, useRef, useState } from 'react';
import { Check, Copy } from 'lucide-react';
import { t, useLocale } from '../i18n';

export function CopyButton({ value, label }: { value: string; label?: string }) {
  useLocale();
  const [state, setState] = useState<'idle' | 'copied' | 'failed'>('idle');
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const copy = async () => {
    clearTimeout(timer.current);
    try { await navigator.clipboard.writeText(value); setState('copied'); }
    catch { setState('failed'); }
    timer.current = setTimeout(() => setState('idle'), 2000);
  };
  return <span className="copy-control"><button type="button" className="subtle" aria-label={label ?? t('workspace-copy')} onClick={() => { void copy(); }}>
    {state === 'copied' ? <Check size={13} /> : <Copy size={13} />}{state === 'copied' ? t('workspace-copied') : label ?? t('workspace-copy')}
  </button>{state === 'failed' && <span role="status" className="error">{t('ui-copy-failed')}</span>}</span>;
}
