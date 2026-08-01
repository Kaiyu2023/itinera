import type { ReactNode } from 'react';
import { createPortal } from 'react-dom';
import type { Place } from '../api/types';
import { useI18n } from '../i18n';
import { PlaceGuide } from './PlaceGuide';
import { PlacePhotoBanner } from './PlacePhotoBanner';
import { SheetModal } from './SheetModal';

/** Full place context for compact surfaces that cannot safely expand in place. */
export function PlaceGuideDialog({
  place,
  tripContext,
  contextLabel,
  onClose,
}: {
  place: Place;
  tripContext?: ReactNode;
  contextLabel?: string;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const titleId = `place-guide-dialog-${place.id}`;

  return createPortal(
    <SheetModal onClose={onClose}>
      <div className="exp-modal place-guide-dialog" role="dialog" aria-modal="true" aria-labelledby={titleId}>
        <header className="mtop">
          <span className="place-guide-dialog-title">
            <small>{t('plan.guide.placeGuide')}</small>
            <h2 id={titleId}>{place.name}</h2>
          </span>
          <button
            type="button"
            className="x"
            onClick={onClose}
            aria-label={t('plan.guide.close', { place: place.name })}
          >
            ×
          </button>
        </header>
        <div className="place-guide-dialog-scroll">
          <PlacePhotoBanner place={place} />
          <div className="place-guide-dialog-copy">
            <p className="place-guide-dialog-meta">
              {place.city}
              {place.rating != null && <> · ★ {place.rating.toFixed(1)}</>}
            </p>
            <PlaceGuide place={place} tripContext={tripContext} contextLabel={contextLabel} variant="full" />
          </div>
        </div>
      </div>
    </SheetModal>,
    document.body,
  );
}
