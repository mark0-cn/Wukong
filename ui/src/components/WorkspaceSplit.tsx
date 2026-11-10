import { useEffect, useState, type ReactNode } from 'react';
import { Group, Panel, Separator, useDefaultLayout } from 'react-resizable-panels';

const storage = {
  getItem(key: string) { try { return localStorage.getItem(key); } catch { return null; } },
  setItem(key: string, value: string) { try { localStorage.setItem(key, value); } catch { /* Layout stays usable without persistence. */ } },
};

export function useWideWorkspace() {
  const [wide, setWide] = useState(() => window.matchMedia('(min-width: 1100px)').matches);
  useEffect(() => {
    const query = window.matchMedia('(min-width: 1100px)');
    const change = () => setWide(query.matches);
    query.addEventListener('change', change);
    return () => query.removeEventListener('change', change);
  }, []);
  return wide;
}

/** Use the same accessible splitter as shadcn/ui, without importing another UI system. */
export function WorkspaceSplit({ id, label, side, children, visible, leading = false }: {
  id: string; label: string; side: ReactNode; children: ReactNode; visible: boolean; leading?: boolean;
}) {
  const mainId = `${id}-main`, sideId = `${id}-side`;
  const panelIds = visible ? leading ? [sideId, mainId] : [mainId, sideId] : [mainId];
  const { defaultLayout, onLayoutChanged } = useDefaultLayout({ id: `wukong-layout-${id}`, panelIds, storage, onlySaveAfterUserInteractions: true });
  const aside = <Panel key={sideId} id={sideId} className="split-content" defaultSize={leading ? 224 : 268}
    minSize={leading ? 180 : 220} maxSize={leading ? 320 : 400} groupResizeBehavior="preserve-pixel-size">{side}</Panel>;
  const separator = <Separator key={`${id}-separator`} className="workspace-separator" aria-label={label} />;
  return <Group className="workspace-split" id={id} orientation="horizontal" defaultLayout={defaultLayout} onLayoutChanged={onLayoutChanged}>
    {visible && leading && aside}{visible && leading && separator}
    <Panel key={mainId} id={mainId} className="split-content" minSize={leading ? 520 : 320}>{children}</Panel>
    {visible && !leading && separator}{visible && !leading && aside}
  </Group>;
}
