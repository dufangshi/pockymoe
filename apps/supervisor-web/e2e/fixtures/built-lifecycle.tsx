import { useState } from 'react';
import { Dialog, DialogContent, DialogTitle, useVisibleClock } from '@pockymoe/thread-ui';
import { DeviceMonitor } from '../../src/components/DeviceMonitor';

export function BuiltLifecycle() {
  const [open, setOpen] = useState(false);
  const [element, ref] = useState<HTMLSpanElement | null>(null);
  const now = useVisibleClock(true, 1000, element);
  Object.assign(window, { auditCloseDialog: () => setOpen(false) });
  return <main>
    <button onClick={() => setOpen(true)}>Open audit dialog</button>
    <Dialog open={open} onOpenChange={setOpen}><DialogContent aria-describedby={undefined}><DialogTitle>Audit dialog</DialogTitle><button onClick={() => setOpen(false)}>Close audit dialog</button></DialogContent></Dialog>
    <DeviceMonitor />
    <div id="audit-clock-scroll" style={{ height: 100, overflow: 'auto' }}><div style={{ height: 400 }} /><span ref={ref} id="audit-clock">{now}</span></div>
    <div id="audit-follow" style={{ height: 100, overflow: 'auto' }}><div style={{ height: 400 }} /></div>
  </main>;
}
