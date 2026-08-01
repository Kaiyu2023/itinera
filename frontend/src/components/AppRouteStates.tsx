import { useI18n } from '../i18n';

export function AppLoading() {
  const { t } = useI18n();

  return (
    <main className="app-state" role="status" aria-live="polite">
      <div className="app-state-card">
        <span className="app-state-mark loading" aria-hidden>
          I
        </span>
        <p>{t('app.loading')}</p>
      </div>
    </main>
  );
}

export function AppRouteError() {
  return <AppErrorState onRetry={() => window.location.reload()} />;
}

export function AppErrorState({ onRetry }: { onRetry: () => void }) {
  const { t } = useI18n();

  return (
    <main className="app-state" role="alert">
      <div className="app-state-card error">
        <span className="app-state-mark" aria-hidden>
          !
        </span>
        <h1>{t('app.error.title')}</h1>
        <p className="muted">{t('app.error.body')}</p>
        <button type="button" className="btn primary" onClick={onRetry}>
          {t('app.error.reload')}
        </button>
      </div>
    </main>
  );
}
