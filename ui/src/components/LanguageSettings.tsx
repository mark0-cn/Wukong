import { useState } from 'react';
import { errorText } from '../bridge';
import { saveLanguage, t, usePreferences, type LanguageChoice } from '../i18n';

/** Never enters the workspace config save/reload path, even during analysis. */
export function LanguageSettings() {
  const preferences = usePreferences();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState('');
  const change = async (field: 'ui_language' | 'output_language', value: LanguageChoice) => {
    setPending(true); setError('');
    try { await saveLanguage({ ui_language: preferences.ui_language, output_language: preferences.output_language, [field]: value }); }
    catch (error) { setError(errorText(error)); }
    finally { setPending(false); }
  };
  return <section className="language-settings">
    <div className="settings-grid">{(['ui_language', 'output_language'] as const).map(field =>
      <label className="field" key={field}><span>{t(field === 'ui_language' ? 'language-ui' : 'language-output')}</span>
        <select aria-label={t(field === 'ui_language' ? 'language-ui' : 'language-output')} value={preferences[field]} disabled={pending}
          onChange={event => { void change(field, event.target.value as LanguageChoice); }}>
          <option value="auto">{t(field === 'ui_language' ? 'language-auto' : 'language-match-task')}</option>
          <option value="zh-CN">简体中文</option><option value="en-US">English</option>
        </select>
      </label>)}</div>
    <p className="muted">{t('language-help')}</p>
    {error && <p role="alert" className="error">{error}</p>}
  </section>;
}
