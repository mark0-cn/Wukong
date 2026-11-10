import { useEffect, useState } from 'react';
import { api, errorText } from '../bridge';

export function StorageLocations({ epoch, disabled, onChanged }: { epoch: number; disabled: boolean; onChanged: () => void }) {
  const [data, setData] = useState('');
  const [cache, setCache] = useState('');
  const [working, setWorking] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  useEffect(() => {
    let alive = true;
    void api<{ data_dir: string; cache_dir: string }>('update_storage', { epoch }).then(value => {
      if (alive) { setData(value.data_dir ?? ''); setCache(value.cache_dir ?? ''); }
    }).catch(e => { if (alive) setError(errorText(e)); });
    return () => { alive = false; };
  }, [epoch]);
  const choose = async (set: (path: string) => void) => {
    try { const path = await api<string | null>('pick_directory'); if (path) set(path); }
    catch (e) { setError(errorText(e)); }
  };
  const save = async (kind: 'data' | 'cache') => {
    if (kind === 'data' && !window.confirm('将切换到所选任务与证据目录，原目录数据保留且不会自动搬移。确认切换？')) return;
    setWorking(true); setError(''); setMessage('');
    try {
      await api(kind === 'data' ? 'update_data_directory' : 'update_cache_directory', { epoch, destination: kind === 'data' ? data : cache });
      setMessage(kind === 'data' ? '数据目录已切换，原目录数据仍然保留。' : '缓存目录已切换，恢复所需文件已复制；原目录文件保留。');
      onChanged();
    } catch (e) { setError(errorText(e)); }
    finally { setWorking(false); }
  };
  return <fieldset disabled={disabled || working} className="workbench-controls"><legend>存储位置</legend>
    <label>任务与证据目录<input value={data} onChange={e => setData(e.target.value)} /></label>
    <p className="muted">选择已有工作区或新目录。原有任务不会自动搬移，可以选择原目录切回。</p>
    <div className="header-actions"><button onClick={() => void choose(setData)}>选择数据目录</button><button disabled={!data} onClick={() => void save('data')}>应用数据目录</button></div>
    <label>更新缓存目录<input value={cache} onChange={e => setCache(e.target.value)} /></label>
    <p className="muted">安装文件与镜像自检日志可放到其他磁盘。请选择空目录；已验证的安装文件和恢复文件会复制过去。Docker 镜像的磁盘位置由 Docker 设置管理。</p>
    <div className="header-actions"><button onClick={() => void choose(setCache)}>选择缓存目录</button><button disabled={!cache} onClick={() => void save('cache')}>应用缓存目录</button></div>
    {working && <p role="status">正在验证并保存，请等待完成…</p>}{message && <p role="status">{message}</p>}{error && <p role="alert" className="error-banner">{error}</p>}
  </fieldset>;
}
