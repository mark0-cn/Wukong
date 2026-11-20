import { t } from './shared/i18n';
import { initializeLanguage, LanguageProvider } from './shared/i18n';
import { Component, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import App from './app/App';
import { CleanupReview } from './features/maintenance/CleanupReview';
import './theme.css';
import './styles.css';
import './workspace-layout.css';

class ErrorBoundary extends Component<{ children: ReactNode }, { error?: string }> {
  state: { error?: string } = {};
  static getDerivedStateFromError(error: Error) { return { error: error.message }; }
  render() { return this.state.error ? <main className="fatal-error"><h1>{t('workspace-could-not-load-the-interface')}</h1><p>{this.state.error}</p><button onClick={() => window.location.reload()}>{t('workspace-reload')}</button></main> : this.props.children; }
}
const cleanup = (window as Window & { __WUKONG_CLEANUP__?: { initialData: boolean } }).__WUKONG_CLEANUP__;
const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: false, networkMode: 'always', refetchOnWindowFocus: false, refetchOnReconnect: false } },
});
void initializeLanguage().finally(() => createRoot(document.getElementById('root')!).render(<QueryClientProvider client={queryClient}><LanguageProvider><ErrorBoundary>{cleanup ? <CleanupReview initialData={cleanup.initialData} /> : <App />}</ErrorBoundary></LanguageProvider></QueryClientProvider>));
