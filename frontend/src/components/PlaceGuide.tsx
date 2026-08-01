import { useId, useState } from 'react';
import type { ReactNode } from 'react';
import type { Place } from '../api/types';
import { useI18n } from '../i18n';

type PlaceGuideVariant = 'full' | 'compact' | 'disclosure';
type PlaceGuideHeading = 'h4' | 'h5';

const INTRO_KEY = {
  sight: 'plan.guide.whyStop',
  activity: 'plan.guide.whyStop',
  food: 'plan.guide.whyTry',
  lodging: 'plan.guide.aboutStay',
  transport_hub: 'plan.guide.aboutConnection',
} as const;

type ActivityIdea = NonNullable<Place['guide']>['activityIdeas'][number];

function ActivityIdeaItem({ idea }: { idea: ActivityIdea }) {
  const { t } = useI18n();
  const [expanded, setExpanded] = useState(false);
  const detailsId = useId();
  const details = idea.details?.trim();

  return (
    <li className={details ? 'has-details' : undefined}>
      <div className="pg-idea-row">
        {details ? (
          <button
            type="button"
            className="pg-idea-toggle"
            aria-expanded={expanded}
            aria-controls={detailsId}
            aria-label={t(expanded ? 'plan.guide.hideIdeaDetails' : 'plan.guide.showIdeaDetails', {
              idea: idea.title,
            })}
            onClick={() => setExpanded((open) => !open)}
          >
            <span aria-hidden>{expanded ? '−' : '+'}</span>
          </button>
        ) : (
          <span className="pg-idea-bullet" aria-hidden>
            •
          </span>
        )}
        <span className="pg-idea-title">{idea.title}</span>
      </div>
      {details && (
        <p id={detailsId} className="pg-idea-details" hidden={!expanded}>
          {details}
        </p>
      )}
    </li>
  );
}

function GuideDetails({ place, headingLevel }: { place: Place; headingLevel: PlaceGuideHeading }) {
  const { t } = useI18n();
  const guide = place.guide;
  const hasFacts = !!(place.openingHours?.weekdayText.length || place.address || place.website || place.phone);
  const Heading = headingLevel;

  return (
    <div className="pg-detail-sections">
      {!!guide?.activityIdeas.length && (
        <section className="pg-section pg-activity-section">
          <div className="pg-section-head">
            <Heading>{t('plan.guide.ideas')}</Heading>
            <span>{t('plan.guide.pickWhatFits')}</span>
          </div>
          <p className="pg-pool-note">{t('plan.guide.poolNote')}</p>
          <ul className="pg-ideas">
            {guide.activityIdeas.map((idea, index) => (
              <ActivityIdeaItem key={`${idea.title}-${index}`} idea={idea} />
            ))}
          </ul>
        </section>
      )}

      {guide?.intro && (
        <section className="pg-section pg-intro-section">
          <Heading>{t(INTRO_KEY[place.kind])}</Heading>
          <p>{guide.intro}</p>
        </section>
      )}

      {!!guide?.practicalTips.length && (
        <section className="pg-section">
          <Heading>{t('plan.guide.goodToKnow')}</Heading>
          <ul className="pg-tips">
            {guide.practicalTips.map((tip) => (
              <li key={tip}>{tip}</li>
            ))}
          </ul>
        </section>
      )}

      {hasFacts && (
        <section className="pg-section pg-facts-section">
          <Heading>{t('plan.guide.practicalDetails')}</Heading>
          <dl className="pg-facts">
            {place.openingHours?.weekdayText.map((hours, index) => (
              <div key={`${hours}-${index}`}>
                <dt>{t(index === 0 ? 'plan.guide.hours' : 'plan.guide.also')}</dt>
                <dd>{hours}</dd>
              </div>
            ))}
            {place.address && (
              <div>
                <dt>{t('plan.guide.address')}</dt>
                <dd>{place.address}</dd>
              </div>
            )}
            {place.website && (
              <div>
                <dt>{t('plan.guide.website')}</dt>
                <dd>
                  <a href={place.website} target="_blank" rel="noreferrer">
                    {t('plan.guide.officialSite')}
                  </a>
                </dd>
              </div>
            )}
            {place.phone && (
              <div>
                <dt>{t('plan.guide.phone')}</dt>
                <dd>{place.phone}</dd>
              </div>
            )}
          </dl>
        </section>
      )}
    </div>
  );
}

/**
 * Progressive place context shared by planned stops and trip ideas.
 * Global guide copy remains separate from the trip-specific note or pitch.
 */
export function PlaceGuide({
  place,
  tripContext,
  contextLabel,
  variant = 'full',
  headingLevel = 'h4',
}: {
  place: Place;
  tripContext?: ReactNode;
  contextLabel?: string;
  variant?: PlaceGuideVariant;
  headingLevel?: PlaceGuideHeading;
}) {
  const { t } = useI18n();
  const guide = place.guide;
  const summary = guide?.summary?.trim();
  const resolvedContextLabel = contextLabel ?? t('plan.guide.forTrip');

  if (variant === 'compact') {
    if (!summary && !tripContext && !guide?.activityIdeas.length) return null;
    return (
      <section className="place-guide compact" aria-label={t('plan.guide.label', { place: place.name })}>
        {summary && <p className="pg-summary">{summary}</p>}
        {!!guide?.activityIdeas.length && (
          <ul className="pg-idea-chips" aria-label={t('plan.guide.ideas')}>
            {guide.activityIdeas.slice(0, 2).map((idea) => (
              <li key={idea.title}>{idea.title}</li>
            ))}
          </ul>
        )}
        {tripContext && (
          <div className="pg-context compact-context">
            <span>{resolvedContextLabel}</span>
            <div>{tripContext}</div>
          </div>
        )}
      </section>
    );
  }

  const body = <GuideDetails place={place} headingLevel={headingLevel} />;
  return (
    <section className={`place-guide ${variant}`} aria-label={t('plan.guide.label', { place: place.name })}>
      {summary && <p className="pg-summary">{summary}</p>}
      {tripContext && (
        <div className="pg-context">
          <span>{resolvedContextLabel}</span>
          <div>{tripContext}</div>
        </div>
      )}
      {variant === 'disclosure' ? (
        <details className="pg-more">
          <summary>{t('plan.guide.explore')}</summary>
          {body}
        </details>
      ) : (
        body
      )}
    </section>
  );
}
