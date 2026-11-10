import { t, useLocale } from '../i18n';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Copy } from 'lucide-react';
import { useState } from 'react';
import { openLink } from '../bridge';
import { Modal } from './Modal';

function Code({ text, language }: { text: string; language?: string }) {
  useLocale();
  const [copied, setCopied] = useState(false);
  return <div className="code-block"><div className="code-heading"><span>{language ?? t('workspace-code')}</span>
    <button onClick={() => { void navigator.clipboard.writeText(text).then(() => { setCopied(true); window.setTimeout(() => setCopied(false), 1600); }); }}><Copy size={12} />{copied ? t('workspace-copied') : t('workspace-copy')}</button></div>
    <pre><code>{text}</code></pre></div>;
}
const HTTP_LINK = /^https?:\/\//i;
export function Markdown({ text }: { text: string }) {
  useLocale();
  // Agent/tool content is untrusted: opening a URL must be an explicit,
  // confirmed user decision with the destination visible.
  const [pending, setPending] = useState<string | null>(null);
  return <div className="markdown"><ReactMarkdown remarkPlugins={[remarkGfm]} components={{
    a: ({ href, children }) => <a href={href} onClick={event => {
      event.preventDefault();
      if (href && HTTP_LINK.test(href)) setPending(href);
    }}>{children}</a>,
    img: ({ alt }) => <span className="muted">{t('workspace-image')}{alt ?? t('workspace-attachment')}]</span>,
    pre: ({ children }) => <>{children}</>,
    code: ({ className, children, node }) => {
      const text = String(children).replace(/\n$/, '');
      const block = Boolean(className) || Boolean(node?.position && node.position.end.line > node.position.start.line);
      return block ? <Code text={text} language={className?.replace('language-', '')} /> : <code>{children}</code>;
    },
  }}>{text}</ReactMarkdown>
    {pending && <Modal title={t('workspace-open-external-link')} description={t('workspace-this-link-opens-in-your-system-browser')} onClose={() => setPending(null)}>
      <p className="muted">{t('workspace-this-content-comes-from-analysis-check-the')}</p>
      <pre className="link-target">{pending}</pre>
      <div className="modal-actions">
        <button className="subtle" onClick={() => setPending(null)}>{t('workspace-cancel')}</button>
        <button className="primary" onClick={() => { const url = pending; setPending(null); void openLink(url).catch(() => {}); }}>{t('workspace-open-in-browser')}</button>
      </div>
    </Modal>}
  </div>;
}
