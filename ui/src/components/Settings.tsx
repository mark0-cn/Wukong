import { useEffect, useState } from 'react';
import { FileCode2, FolderOpen, RotateCcw, Save } from 'lucide-react';
import { api, errorText } from '../bridge';
import type { ConfigDocument, ConfigEdit, Host, Json } from '../types';
import { Modal } from './Modal';

type Field = { path: string; label: string; kind?: 'number' | 'checkbox' | 'provider' | 'backend'; help?: string };
const groups: { title: string; fields: Field[] }[] = [
  { title: '模型', fields: [
    { path: 'model.provider', label: '服务商', kind: 'provider' }, { path: 'model.model', label: '主模型' },
    { path: 'model.role_model', label: '协作 Agent 模型', help: '留空时使用主模型' },
    { path: 'model.base_url', label: 'API 地址', help: '留空时使用服务商默认地址' },
    { path: 'model.api_key_env', label: '密钥环境变量名', help: '填写变量名称，密钥由本地环境读取' },
    { path: 'model.max_tokens', label: '单次输出 tokens', kind: 'number' },
    { path: 'model.request_timeout_secs', label: '请求超时（秒）', kind: 'number' },
  ] },
  { title: '任务预算', fields: [
    { path: 'budget.max_total_tokens', label: '总 tokens', kind: 'number' },
    { path: 'budget.max_turns_per_agent', label: '每个 Agent 最大轮数', kind: 'number' },
    { path: 'budget.max_agents', label: '最大 Agent 数', kind: 'number' },
    { path: 'budget.max_concurrent_agents', label: '同时运行 Agent 数', kind: 'number' },
    { path: 'budget.max_depth', label: '最大协作层级', kind: 'number' },
    { path: 'budget.max_wall_seconds', label: '任务时限（秒）', kind: 'number' },
  ] },
  { title: '本地环境', fields: [
    { path: 'state_dir', label: '任务与证据目录' },
    { path: 'analysis.ghidra_executable', label: 'Ghidra 可执行文件' },
    { path: 'runtime.backend', label: '动态分析后端', kind: 'backend' },
    { path: 'runtime.docker_container', label: 'Docker 容器' },
    { path: 'runtime.ssh_host', label: 'SSH 主机' },
    { path: 'runtime.python', label: 'Python 可执行文件' },
    { path: 'runtime.bridge_path', label: '运行时桥接程序路径' },
    { path: 'execution.stream', label: '模型流式输出', kind: 'checkbox' },
    { path: 'execution.summarize_tool_outputs', label: '压缩模型上下文中的大型工具输出', kind: 'checkbox' },
  ] },
];
function atPath(form: Record<string, Json>, path: string): Json | undefined {
  let value: Json | undefined = form;
  for (const key of path.split('.')) value = value && typeof value === 'object' && !Array.isArray(value) ? value[key] : undefined;
  return value;
}
export function Settings({ open, host, onClose, onSaved, onDirty }: {
  open: boolean; host: Host; onClose: () => void; onSaved: () => void; onDirty: (value: boolean) => void;
}) {
  const [document, setDocument] = useState<ConfigDocument>();
  const [loadedEpoch, setLoadedEpoch] = useState(host.epoch);
  const [edits, setEdits] = useState<Record<string, Json>>({});
  const [raw, setRaw] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const dirty = Boolean(document && (document.source !== (document.original ?? '') || Object.keys(edits).length));
  useEffect(() => { onDirty(dirty); }, [dirty, onDirty]);
  useEffect(() => {
    if (!open || document) return;
    let alive = true;
    setBusy(true); setError('');
    void api<ConfigDocument>('config_read', { epoch: host.epoch }).then(value => { if (alive) { setBusy(false); setDocument(value); setLoadedEpoch(host.epoch); } })
      .catch(error => { if (alive) setError(errorText(error)); }).finally(() => { if (alive) setBusy(false); });
    return () => { alive = false; };
  }, [open, document, host.epoch]);
  const patch = (): ConfigEdit[] => Object.entries(edits).map(([path, value]) => ({ path: path.split('.'), value }));
  const load = async (choose: boolean) => {
    if (dirty && !window.confirm('放弃当前设置草稿并重新加载？')) return;
    setBusy(true); setError('');
    try {
      const value = await api<ConfigDocument | null>(choose ? 'config_open' : 'config_read', { epoch: host.epoch });
      if (value) { setDocument(value); setEdits({}); setLoadedEpoch(host.epoch); }
    } catch (error) { setError(errorText(error)); }
    finally { setBusy(false); }
  };
  const switchMode = async () => {
    if (!document) return;
    setBusy(true); setError('');
    try {
      const value = await api<ConfigDocument>('config_preview', { document, edits: patch() });
      setDocument(value); setEdits({}); setRaw(value => !value);
    } catch (error) { setError(errorText(error)); }
    finally { setBusy(false); }
  };
  const save = async () => {
    if (!document) return;
    setBusy(true); setError('');
    try {
      const value = await api<ConfigDocument>('config_save', { epoch: loadedEpoch, document, edits: patch() });
      setDocument(value); setEdits({}); onDirty(false); onSaved(); onClose();
      // A later opening reads the current document and workspace epoch.
      setDocument(undefined);
    } catch (error) { setError(errorText(error)); }
    finally { setBusy(false); }
  };
  if (!open) return null;
  return <Modal title="工作区设置" description="新任务使用当前配置；恢复任务沿用历史配置。关闭窗口会保留设置草稿。" onClose={onClose} wide>
    <div className="settings-toolbar"><span title={document?.path ?? host.config_path}><FileCode2 size={14} />{document?.path ?? host.config_path}</span>
      <button disabled={busy} onClick={() => { void load(true); }}><FolderOpen size={14} />打开配置</button>
      <button disabled={busy} onClick={() => { void load(false); }}><RotateCcw size={14} />重新加载</button>
      <button disabled={busy || !document} onClick={() => { void switchMode(); }}>{raw ? '表单' : 'TOML 源码'}</button></div>
    {loadedEpoch !== host.epoch && <p className="error" role="alert">工作区配置已变化，请重新加载设置。草稿尚未保存。</p>}
    {error && <p className="error" role="alert">{error}</p>}
    <div className="settings-scroll">{!document ? <p className="muted">{busy ? '正在读取配置…' : '配置读取失败，请重新加载。'}</p> : raw ?
      <textarea className="toml-editor" aria-label="TOML 配置源码" spellCheck={false} value={document.source} disabled={busy}
        onChange={event => setDocument({ ...document, source: event.target.value })} /> : groups.map(group => <section key={group.title} className="settings-group"><h3>{group.title}</h3>
        <div className="settings-grid">{group.fields.map(field => {
          const value = Object.hasOwn(edits, field.path) ? edits[field.path] : atPath(document.form, field.path);
          const change = (value: Json) => setEdits(previous => ({ ...previous, [field.path]: value }));
          return <label key={field.path} className={`field ${field.kind === 'checkbox' ? 'field-checkbox' : ''}`}><span>{field.label}</span>
            {field.kind === 'provider' || field.kind === 'backend' ? <select value={String(value ?? '')} disabled={busy} onChange={event => change(event.target.value)}>{(field.kind === 'provider' ? ['openai', 'openai_responses', 'anthropic', 'ollama'] : ['disabled', 'docker', 'ssh', 'trusted_local']).map(option => <option key={option} value={option}>{option}</option>)}</select> :
              field.kind === 'checkbox' ? <input type="checkbox" checked={Boolean(value)} disabled={busy} onChange={event => change(event.target.checked)} /> :
                <input type={field.kind === 'number' ? 'number' : 'text'} min={field.kind === 'number' ? 1 : undefined} value={value == null ? '' : String(value)} disabled={busy}
                  onChange={event => change(field.kind === 'number' ? event.target.value === '' ? '' : Number(event.target.value) : event.target.value || null)} />}
            {field.help && <small>{field.help}</small>}</label>;
        })}</div></section>)}</div>
    <div className="modal-footer"><span className="muted">{dirty ? '有未保存的修改' : '配置已加载'} · 注释与相对路径会保留</span>
      <button onClick={onClose}>关闭</button><button className="primary" disabled={busy || !document || loadedEpoch !== host.epoch || Boolean(host.execution) || host.pending_operations > 0} onClick={() => { void save(); }}><Save size={14} />{busy ? '处理中…' : '保存并应用'}</button></div>
    {(host.execution || host.pending_operations > 0) && <p className="muted">请先暂停分析并等待正在进行的操作结束，再应用配置。</p>}
  </Modal>;
}
