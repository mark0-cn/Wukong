import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Copy } from 'lucide-react';
import { useState } from 'react';
import { openLink } from '../bridge';

function Code({ text, language }: { text: string; language?: string }) {
  const [copied, setCopied] = useState(false);
  return <div className="code-block"><div className="code-heading"><span>{language ?? '代码'}</span>
    <button onClick={() => { void navigator.clipboard.writeText(text).then(() => { setCopied(true); window.setTimeout(() => setCopied(false), 1600); }); }}><Copy size={12} />{copied ? '已复制' : '复制'}</button></div>
    <pre><code>{text}</code></pre></div>;
}
export function Markdown({ text }: { text: string }) {
  return <div className="markdown"><ReactMarkdown remarkPlugins={[remarkGfm]} components={{
    a: ({ href, children }) => <a href={href} onClick={event => { event.preventDefault(); if (href) void openLink(href).catch(() => {}); }}>{children}</a>,
    img: ({ alt }) => <span className="muted">[图片：{alt ?? '附件'}]</span>,
    pre: ({ children }) => <>{children}</>,
    code: ({ className, children, node }) => {
      const text = String(children).replace(/\n$/, '');
      const block = Boolean(className) || Boolean(node?.position && node.position.end.line > node.position.start.line);
      return block ? <Code text={text} language={className?.replace('language-', '')} /> : <code>{children}</code>;
    },
  }}>{text}</ReactMarkdown></div>;
}
