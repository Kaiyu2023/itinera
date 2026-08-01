import { useState } from 'react';
import type { Place } from '../api/types';
import { useI18n } from '../i18n';
import { Lightbox } from './PlaceThumb';

/** Shared hero/gallery trigger for the timeline and map place-detail surfaces. */
export function PlacePhotoBanner({ place }: { place: Place }) {
  const { t } = useI18n();
  const [viewer, setViewer] = useState<number | null>(null);
  const photos = place.photoUrls;
  if (photos.length === 0) return null;

  return (
    <>
      <button
        type="button"
        className="photo-banner"
        onClick={() => setViewer(0)}
        aria-label={
          photos.length > 1
            ? t('plan.photo.viewMany', { count: photos.length, place: place.name })
            : t('plan.photo.viewOne', { place: place.name })
        }
      >
        <img src={photos[0]} alt="" />
        {photos.length > 1 && (
          <span className="thumb-more" aria-hidden="true">
            <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.4">
              <rect x="3.5" y="3.5" width="7" height="7" rx="1.5" />
              <path d="M8.5 1.5h-6A1.5 1.5 0 0 0 1 3v6" />
            </svg>
            {photos.length}
          </span>
        )}
      </button>
      {viewer != null && (
        <Lightbox
          photos={photos}
          name={place.name}
          index={viewer}
          onIndex={setViewer}
          onClose={() => setViewer(null)}
        />
      )}
    </>
  );
}
