import { useCallback, useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { useIsDesktop } from './hooks';
import { useModalChrome } from './useModalChrome';

/**
 * Centered modal on desktop, bottom sheet on phones — the app's composer
 * chrome, shared by the ledger's add-expense / settle-up and prep's
 * create-notice surfaces. The child brings its own `.exp-modal` card (which
 * turns into a bottom sheet under the mobile breakpoint via CSS).
 *
 * Close is orchestrated internally: a backdrop click or Escape flags `closing`,
 * which swaps in the exit animation; when the backdrop's own animation ends we
 * call `onClose`. So every dismissal (fade+scale on desktop, slide-down on
 * mobile) animates out before the surface unmounts.
 *
 * Escape respects the app's Escape-stacking convention: a photo lightbox
 * (`.lb-backdrop`) owns Escape while it's up, so we only claim it otherwise
 * (mirrors the Plan-tab governance modal).
 */
export function SheetModal({ onClose, children }: { onClose: () => void; children: ReactNode }) {
  const isDesktop = useIsDesktop();
  const chrome = useModalChrome<HTMLDivElement>();
  const [closing, setClosing] = useState(false);
  const requestClose = useCallback(() => setClosing(true), []);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !document.querySelector('.lb-backdrop')) requestClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [requestClose]);

  return (
    <div
      className={`gov-backdrop${closing ? ' closing' : ''}`}
      onClick={requestClose}
      onAnimationEnd={(e) => {
        if (closing && e.target === e.currentTarget) onClose();
      }}
    >
      {/* `display: contents` on mobile keeps the sheet's own layout, and does
          not affect DOM containment — which is all the focus trap needs. */}
      <div ref={chrome} onClick={(e) => e.stopPropagation()} style={{ display: isDesktop ? 'block' : 'contents' }}>
        {children}
      </div>
    </div>
  );
}
