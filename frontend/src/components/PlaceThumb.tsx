import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { useModalChrome } from './useModalChrome';

/**
 * Thumbnail for a place's first photo. When the place has more than one,
 * a count chip signals it; clicking opens a lightbox that can be paged
 * with the arrow buttons, arrow keys, or a horizontal swipe.
 */
export function PlaceThumb({ photos, name }: { photos: string[]; name: string }) {
  const [viewer, setViewer] = useState<number | null>(null);

  if (photos.length === 0) return null;

  return (
    <>
      <button
        type="button"
        className="thumb-btn"
        onClick={() => setViewer(0)}
        aria-label={photos.length > 1 ? `View ${photos.length} photos of ${name}` : `View photo of ${name}`}
      >
        <img className="thumb" src={photos[0]} alt={name} loading="lazy" />
        {photos.length > 1 && (
          <span className="thumb-more" aria-hidden="true">
            <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.4">
              <rect x="3.5" y="3.5" width="7" height="7" rx="1.5" />
              <path d="M8.5 1.5h-6a1.5 1.5 0 0 0-1.5 1.5v6" />
            </svg>
            {photos.length}
          </span>
        )}
      </button>
      {viewer != null && (
        <Lightbox photos={photos} name={name} index={viewer} onIndex={setViewer} onClose={() => setViewer(null)} />
      )}
    </>
  );
}

export function Lightbox({
  photos,
  name,
  index,
  onIndex,
  onClose,
}: {
  photos: string[];
  name: string;
  index: number;
  onIndex: (i: number) => void;
  onClose: () => void;
}) {
  const touchX = useRef<number | null>(null);
  const many = photos.length > 1;
  const prev = () => onIndex((index - 1 + photos.length) % photos.length);
  const next = () => onIndex((index + 1) % photos.length);

  /**
   * The lightbox used to focus its own close button and lock body scroll, and
   * stop there: Tab past the close button walked straight into the page behind
   * the scrim, and closing dumped focus on <body> instead of the thumbnail that
   * opened it. It now shares the app's modal chrome — which traps Tab, locks
   * scroll once (nested surfaces don't each restore the scrollbar padding), and
   * restores focus to the opener.
   *
   * `useModalChrome` deliberately doesn't touch Escape, because Escape is
   * stacked here: this backdrop sits *above* a composer that also listens for
   * it, so the keydown below stops propagation to make sure the photo viewer —
   * the topmost surface — is the only thing that closes.
   */
  const dialogRef = useModalChrome<HTMLDivElement>();

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        onClose();
      }
      if (many && e.key === 'ArrowLeft') prev();
      if (many && e.key === 'ArrowRight') next();
    };
    // Capture phase: the composer underneath binds Escape on `window` too, and
    // whoever is on top has to win regardless of bind order.
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  });

  return createPortal(
    <div
      ref={dialogRef}
      className="lb-backdrop"
      role="dialog"
      aria-modal="true"
      aria-label={`Photos of ${name}`}
      tabIndex={-1}
      onClick={onClose}
      onTouchStart={(e) => {
        touchX.current = e.touches[0].clientX;
      }}
      onTouchEnd={(e) => {
        if (!many || touchX.current == null) return;
        const dx = e.changedTouches[0].clientX - touchX.current;
        touchX.current = null;
        if (dx > 48) prev();
        else if (dx < -48) next();
      }}
    >
      <figure className="lb-stage" onClick={(e) => e.stopPropagation()}>
        <img
          key={photos[index]}
          className="lb-img"
          src={photos[index]}
          alt={`${name} — photo ${index + 1} of ${photos.length}`}
        />
        <figcaption className="lb-cap">
          <span>{name}</span>
          {many && (
            <span className="lb-count">
              {index + 1} / {photos.length}
            </span>
          )}
        </figcaption>
        {many && (
          <div className="lb-dots" aria-hidden="true">
            {photos.map((p, i) => (
              <span key={p} className={`lb-dot${i === index ? ' active' : ''}`} />
            ))}
          </div>
        )}
        {many && (
          <>
            <button type="button" className="lb-nav prev" onClick={prev} aria-label="Previous photo">
              ‹
            </button>
            <button type="button" className="lb-nav next" onClick={next} aria-label="Next photo">
              ›
            </button>
          </>
        )}
        <button type="button" className="lb-close" onClick={onClose} aria-label="Close photo viewer">
          ×
        </button>
      </figure>
    </div>,
    document.body,
  );
}
