import { setThemeChoice, useThemeChoice, THEME_CHOICES } from '../theme/useTheme';
import type { ThemeChoice } from '../theme/useTheme';
import { useI18n, type MessageKey } from '../i18n';
import { MoonGlyph, SunGlyph } from './SkyGlyph';

/**
 * Light / Auto / Dark.
 *
 * Three states, not two, and `Auto` sits in the middle because it is the
 * default and because the control then reads as a slider from light to dark
 * with "let the device decide" at the neutral point.
 *
 * A two-state toggle would have been smaller, but it cannot express "follow my
 * phone", and a phone that switches at sunset is doing something this app has
 * opinions about — the whole plan view is drawn around when the sun goes down.
 */
const LABEL: Record<ThemeChoice, MessageKey> = {
  light: 'theme.light',
  system: 'theme.auto',
  dark: 'theme.dark',
};

export function ThemeToggle() {
  const choice = useThemeChoice();
  const { t } = useI18n();
  return (
    <div className="theme-seg" role="radiogroup" aria-label={t('theme.groupLabel')}>
      {THEME_CHOICES.map((c) => (
        <button
          key={c}
          type="button"
          role="radio"
          aria-checked={choice === c}
          className={choice === c ? 'active' : ''}
          onClick={() => setThemeChoice(c)}
          title={t(c === 'system' ? 'theme.followDevice' : c === 'light' ? 'theme.alwaysLight' : 'theme.alwaysDark')}
        >
          {c === 'light' && <SunGlyph />}
          {c === 'dark' && <MoonGlyph />}
          {c === 'system' && (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.7} aria-hidden>
              <rect x="2.6" y="4.2" width="18.8" height="13" rx="2" />
              <path d="M8.4 20.4h7.2" strokeLinecap="round" />
            </svg>
          )}
          <span>{t(LABEL[c])}</span>
        </button>
      ))}
    </div>
  );
}
