import { t } from './i18n';
import { initializeLanguage, LanguageProvider } from './i18n';
import { Component, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import './theme.css';
import './styles.css';
import './workspace-layout.css';

class ErrorBoundary extends Component<{ children: ReactNode }, { error?: string }> {
  state: { error?: string } = {};
  static getDerivedStateFromError(error: Error) { return { error: error.message }; }
  render() { return this.state.error ? <main className="fatal-error"><h1>{t('workspace-could-not-load-the-interface')}</h1><p>{this.state.error}</p><button onClick={() => window.location.reload()}>{t('workspace-reload')}</button></main> : this.props.children; }
}
void initializeLanguage().finally(() => createRoot(document.getElementById('root')!).render(<LanguageProvider><ErrorBoundary><App /></ErrorBoundary></LanguageProvider>));
