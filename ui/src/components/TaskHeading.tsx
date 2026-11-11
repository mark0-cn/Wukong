import { useState } from 'react';
import { Pencil } from 'lucide-react';
import { t, useLocale } from '../i18n';
import { Modal } from './Modal';
import type { Run } from '../types';
import { fileName, Status } from './Status';

export function TaskHeading({ run, name, rename }: { run: Run; name?: string; rename: (value: string) => void }) {
  useLocale();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');
  return <><div><h1 title={run.target_path}>{name || fileName(run.target_path)}</h1><Status value={run.status} />
    <button className="icon-button rename-task" aria-label={t('ui-rename-task')} onClick={() => { setDraft(name ?? ''); setEditing(true); }}><Pencil size={13} /></button></div>
    {name && <span className="task-filename" title={run.target_path}>{fileName(run.target_path)}</span>}
    {editing && <Modal title={t('ui-rename-task')} description={t('ui-name-local')} onClose={() => setEditing(false)}>
      <form onSubmit={event => { event.preventDefault(); rename(draft.trim()); setEditing(false); }}>
        <label className="field">{t('ui-task-name')}<input autoFocus maxLength={100} value={draft} placeholder={fileName(run.target_path)} onChange={event => setDraft(event.target.value)} /></label>
        <div className="modal-footer"><button type="button" onClick={() => setEditing(false)}>{t('workspace-cancel')}</button><button className="primary" type="submit">{t('ui-save-name')}</button></div>
      </form>
    </Modal>}
  </>;
}
