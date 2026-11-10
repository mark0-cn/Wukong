import { useState } from 'react';
import { api, errorText } from '../bridge';
import type { WorkbenchRequest } from '../commands';
import { Modal } from './Modal';

export function Workbench({ epoch, onClose, onChanged }: { epoch: number; onClose: () => void; onChanged: () => void }) {
  const [id, setId] = useState('');
  const [source, setSource] = useState('');
  const [binary, setBinary] = useState('');
  const [destination, setDestination] = useState('');
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<unknown>();
  const [error, setError] = useState('');
  const run = async (request: WorkbenchRequest) => {
    setBusy(true); setError('');
    try { setResult(await api('workbench_action', { epoch, request })); onChanged(); }
    catch (e) { setError(errorText(e)); }
    finally { setBusy(false); }
  };
  return <Modal title="Pwn 工作环境" description="工具与题目分别运行；每道题使用独立网络。构建和导出可能需要数分钟。" wide onClose={() => { if (!busy) onClose(); }}>
    <fieldset disabled={busy} className="workbench-controls">
      <legend>工具环境</legend>
      <div className="header-actions">
        <button onClick={() => void run({ action: 'status' })}>查看状态</button>
        <button onClick={() => void run({ action: 'up' })}>启动环境</button>
        <button onClick={() => void run({ action: 'doctor' })}>运行自检</button>
        <button onClick={() => void run({ action: 'down' })}>停止环境</button>
        <button onClick={() => void run({ action: 'select', id: null })}>使用工具环境</button>
      </div>
      <label>题目名称<input value={id} onChange={e => setId(e.target.value)} placeholder="例如 heap-demo" /></label>
      <label>附件目录或文件<input value={source} onChange={e => setSource(e.target.value)} placeholder="附件的完整路径" /></label>
      <label>主程序相对路径（存在多个 ELF 时填写）<input value={binary} onChange={e => setBinary(e.target.value)} placeholder="例如 bin/challenge" /></label>
      <div className="header-actions">
        <button onClick={() => void run({ action: 'list' })}>题目列表</button>
        <button disabled={!id || !source} onClick={() => void run({ action: 'import', id, source, binary: binary || null })}>导入题目</button>
        <button disabled={!id} onClick={() => void run({ action: 'build', id })}>构建题目镜像</button>
        <button disabled={!id} onClick={() => void run({ action: 'prepare', id })}>准备运行文件</button>
        <button disabled={!id} onClick={() => void run({ action: 'start', id })}>启动题目</button>
        <button disabled={!id} onClick={() => void run({ action: 'stop', id })}>停止题目</button>
        <button disabled={!id} onClick={() => void run({ action: 'select', id })}>新任务使用此题环境</button>
      </div>
      <p className="muted">服务地址为 challenge:题目端口。网络连接用于输入输出，调试使用准备好的本地文件。切换环境只影响新任务。</p>
      <label>迁移包保存目录<input value={destination} onChange={e => setDestination(e.target.value)} placeholder="填写一个尚不存在的新目录" /></label>
      <button disabled={!destination} onClick={() => void run({ action: 'export', destination })}>导出离线迁移包</button>
    </fieldset>
    {busy && <p role="status">正在处理，请等待操作完成。</p>}
    {error && <p className="error-banner" role="alert">{error}</p>}
    {result !== undefined && <details open><summary>操作结果</summary><pre className="workbench-result">{JSON.stringify(result, null, 2)}</pre></details>}
  </Modal>;
}
