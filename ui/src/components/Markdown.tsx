import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Copy } from 'lucide-react';
import { useState } from 'react';
import { openLink } from '../bridge';
import { Modal } from './Modal';

function Code({ text, language }: { text: string; language?: string }) {
  const [copied, setCopied] = useState(false);
  return <div className="code-block"><div className="code-heading"><span>{language ?? '代码'}</span>
    <button onClick={() => { void navigator.clipboard.writeText(text).then(() => { setCopied(true); window.setTimeout(() => setCopied(false), 1600); }); }}><Copy size={12} />{copied ? '已复制' : '复制'}</button></div>
    <pre><code>{text}</code></pre></div>;
}
const HTTP_LINK = /^https?:\/\//i;
export function Markdown({ text }: { text: string }) {
  // Agent/tool content is untrusted: opening a URL must be an explicit,
  // confirmed user decision with the destination visible.
  const [pending, setPending] = useState<string | null>(null);
  return <div className="markdown"><ReactMarkdown remarkPlugins={[remarkGfm]} components={{
    a: ({ href, children }) => <a href={href} onClick={event => {
      event.preventDefault();
      if (href && HTTP_LINK.test(href)) setPending(href);
    }}>{children}</a>,
    img: ({ alt }) => <span className="muted">[图片：{alt ?? '附件'}]</span>,
    pre: ({ children }) => <>{children}</>,
    code: ({ className, children, node }) => {
      const text = String(children).replace(/\n$/, '');
      const block = Boolean(className) || Boolean(node?.position && node.position.end.line > node.position.start.line);
      return block ? <Code text={text} language={className?.replace('language-', '')} /> : <code>{children}</code>;
    },
  }}>{text}</ReactMarkdown>
    {pending && <Modal title="打开外部链接" description="该链接将用系统浏览器打开" onClose={() => setPending(null)}>
      <p className="muted">内容来自分析过程，打开前请确认目标地址：</p>
      <pre className="link-target">{pending}</pre>
      <div className="modal-actions">
        <button className="subtle" onClick={() => setPending(null)}>取消</button>
        <button className="primary" onClick={() => { const url = pending; setPending(null); void openLink(url).catch(() => {}); }}>在浏览器中打开</button>
      </div>
    </Modal>}
  </div>;
}
