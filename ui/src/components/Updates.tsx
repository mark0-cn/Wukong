import { useEffect, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { api, errorText } from '../bridge';
import { Modal } from './Modal';
import { StorageLocations } from './StorageLocations';

import type { UpdateAction, UpdateStatus } from '../commands';
export type { UpdateStatus } from '../commands';
export function Updates({ epoch, onClose, onChanged }: { epoch: number; onClose: () => void; onChanged: () => void }) {
  const [status, setStatus] = useState<UpdateStatus>();
  const [working, setWorking] = useState(false);
  const [error, setError] = useState('');
  const [storage, setStorage] = useState(false);
  const [confirm, setConfirm] = useState<'install' | 'recover' | 'clean_cache' | 'clean_images'>();
  useEffect(() => {
    let alive = true;
    const refresh = () => { void api('update_action', { action: 'status', epoch }).then(value => { if (alive) setStatus(value); }).catch(e => { if (alive) setError(errorText(e)); }); };
    refresh(); const timer = window.setInterval(refresh, 1000);
    return () => { alive = false; window.clearInterval(timer); };
  }, [epoch]);
  const run = async (action: UpdateAction) => {
    setWorking(true); setError(''); setConfirm(undefined);
    try { setStatus(await api('update_action', { action, epoch })); onChanged(); }
    catch (e) { setError(errorText(e)); }
    finally { setWorking(false); }
  };
  const linux = navigator.userAgent.includes('Linux');
  return <Modal title={status?.needs_setup ? '欢迎使用 Wukong · 准备环境' : '更新与环境'} description="客户端与工具环境按经过验证的版本组合更新。题目环境和分析数据保留。" wide onClose={() => { if (!working) onClose(); }}>
    <div className="update-summary"><div><span className="muted">当前客户端</span><h3>{status?.version ?? '正在读取…'}</h3></div><div><span className="muted">可用版本</span><h3>{status?.available ?? '尚未发现更新'}</h3></div></div>
    {status?.needs_setup && <ol className="setup-steps"><li>安装并启动 Docker，确保能够运行 Linux 容器。</li><li>点击“准备工具环境”，下载此版本配套镜像并完成自检。请为 Docker 至少预留 25 GiB。</li><li>环境就绪后，在模型设置中配置服务，再开始分析。</li></ol>}
    <div className="header-actions"><button onClick={() => void openUrl(linux ? 'https://docs.docker.com/engine/install/' : 'https://docs.docker.com/desktop/setup/install/windows-install/')}>Docker 安装说明</button>{linux && <button disabled={working} onClick={() => void run('integrate')}>安装到应用菜单并重启</button>}</div>
    <p role="status">{status?.message || '可以检查更新，或准备当前版本的工具环境。'}</p>
    {status?.phase === 'downloading' && status.total && <div><progress aria-label="客户端下载进度" max={status.total} value={status.downloaded} /><p className="muted">{(status.downloaded / 1048576).toFixed(1)} / {(status.total / 1048576).toFixed(1)} MiB</p></div>}
    {working && <p className="muted">正在处理，请保持应用打开。镜像下载可在失败后重试，已下载的层会被复用。</p>}
    {(error || status?.phase === 'error') && <p className="error-banner" role="alert">{error || status?.message}</p>}
    {status?.notes && <details><summary>版本说明</summary><p>{status.notes}</p></details>}
    <details onToggle={event => setStorage(event.currentTarget.open)}><summary>配置存储目录</summary>{storage && <StorageLocations epoch={epoch} disabled={working || Boolean(status?.pending)} onChanged={onChanged} />}</details>
    {status?.pending && <p className="error-banner">上一次更新尚未确认。完成恢复前不会启动新的分析任务。</p>}
    <fieldset disabled={working} className="workbench-controls"><legend>版本与环境</legend><div className="header-actions">
      <button onClick={() => void run('check')}>检查更新</button>
      <button disabled={status?.pending} onClick={() => void run('setup')}>{status?.needs_setup ? '准备工具环境' : '自检并修复环境'}</button>
      <button disabled={!status?.available || status.pending || status.needs_setup} onClick={() => void run('prepare')}>下载并验证更新</button>
      <button className="primary" disabled={!status?.prepared || status.pending} onClick={() => setConfirm('install')}>安装并重启</button>
    </div><div className="header-actions"><button disabled={!status?.recovery} onClick={() => setConfirm('recover')}>恢复上一版本</button><button disabled={status?.pending} onClick={() => setConfirm('clean_cache')}>清理旧下载缓存</button><button disabled={status?.pending} onClick={() => setConfirm('clean_images')}>清理旧工具镜像</button></div></fieldset>
    {status?.recovery && <details><summary>无法启动应用时如何恢复</summary><p>关闭 Wukong，运行以下独立恢复程序。恢复使用已验证的本地安装文件。</p><code className="path">{status.recovery}</code></details>}
    {confirm && <div className="notice"><p>{confirm === 'install' ? '安装会关闭并重启客户端，请先保存未发送的草稿。确认继续？' : confirm === 'recover' ? '客户端将退出并打开独立恢复程序，请在恢复窗口中继续。确认恢复上一版本？' : confirm === 'clean_images' ? '仅清理本应用记录的旧工具镜像，保留当前版本、恢复版本和所有容器引用的镜像。确认清理？' : '仅清理旧客户端下载缓存，保留当前版本和恢复版本。确认清理？'}</p><div className="header-actions"><button onClick={() => setConfirm(undefined)}>取消</button><button className="primary" onClick={() => void run(confirm)}>确认</button></div></div>}
  </Modal>;
}
