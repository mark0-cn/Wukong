import * as Dialog from '@radix-ui/react-dialog';
import { X } from 'lucide-react';
import type { ReactNode } from 'react';

export function Modal({ title, description, children, onClose, wide = false }: {
  title: string; description?: string; children: ReactNode; onClose: () => void; wide?: boolean;
}) {
  return <Dialog.Root open onOpenChange={open => { if (!open) onClose(); }}>
    <Dialog.Portal><Dialog.Overlay className="modal-overlay" /><Dialog.Content className={`modal ${wide ? 'modal-wide' : ''}`}>
      <div className="modal-heading"><div><Dialog.Title>{title}</Dialog.Title>
        <Dialog.Description className={description ? 'muted' : 'sr-only'}>{description ?? title}</Dialog.Description></div>
        <Dialog.Close className="icon-button" aria-label="关闭"><X size={18} /></Dialog.Close></div>
      {children}
    </Dialog.Content></Dialog.Portal>
  </Dialog.Root>;
}
