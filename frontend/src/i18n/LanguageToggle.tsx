import { useI18n } from './index';
import styles from './LanguageToggle.module.css';

/** A compact two-language switch that remains usable in the crowded phone bar. */
export function LanguageToggle() {
  const { locale, setLocale, t } = useI18n();
  const next = locale === 'en' ? 'zh-CN' : 'en';
  const nextLanguage = t(next === 'en' ? 'language.english' : 'language.simplifiedChinese');
  const label = t('language.switchTo', { language: nextLanguage });

  return (
    <button type="button" className={styles.toggle} aria-label={label} title={label} onClick={() => setLocale(next)}>
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} aria-hidden>
        <circle cx="12" cy="12" r="8.5" />
        <path d="M3.8 12h16.4M12 3.5c2.2 2.4 3.4 5.3 3.4 8.5S14.2 18.1 12 20.5M12 3.5C9.8 5.9 8.6 8.8 8.6 12s1.2 6.1 3.4 8.5" />
      </svg>
      <span aria-hidden>{locale === 'en' ? 'EN' : '中'}</span>
    </button>
  );
}
