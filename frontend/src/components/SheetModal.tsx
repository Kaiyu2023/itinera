import { useEffect } from 'react';
import type { ReactNode } from 'react';
import { useIsDesktop } from './hooks';

/**
 * Centered modal on desktop, bottom sheet on phones — the app's composer
 * chrome, shared by the ledger's add-expense / settle-up and prep's
 * create-notice surfaces. The child brings its own `.exp-modal` card (which
 * turns into a bottom sheet under the mobile breakpoint via CSS).
 *
 * Escape and a backdrop click both close, respecting the app's Escape-stacking
 * convention: a photo lightbox (`.lb-backdrop`) owns Escape while it's up, so
 * we only claim it otherwise (mirrors the Plan-tab governance modal).
 */
export function SheetModal({ onClose, children }: { onClose: () => void; children: ReactNode }) {
  const isDesktop = useIsDesktop();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !document.querySelector('.lb-backdrop')) onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  return (
    <div className="gov-backdrop" onClick={onClose}>
      <div onClick={(e) => e.stopPropagation()} style={{ display: isDesktop ? 'block' : 'contents' }}>
        {children}
      </div>
    </div>
  );
}
