import { useQueries } from '@tanstack/react-query';
import { useParams } from 'react-router-dom';
import { useApi } from '../api/ApiProvider';
import { formatMoney, useMembers } from '../components/hooks';

export function LedgerTab() {
  const { tripId } = useParams();
  const api = useApi();
  const members = useMembers(tripId);
  const [ledger, trip] = useQueries({
    queries: [
      { queryKey: ['ledger', tripId], queryFn: () => api.getLedger(tripId!), enabled: !!tripId },
      { queryKey: ['trip', tripId], queryFn: () => api.getTrip(tripId!), enabled: !!tripId },
    ],
  });

  if (ledger.isLoading || !ledger.data || !trip.data) return <p className="muted">Loading ledger…</p>;

  const base = trip.data.baseCurrency;
  const maxAbs = Math.max(1, ...ledger.data.balances.map((b) => Math.abs(b.net)));
  const name = (id: string) => members.byId.get(id)?.displayName ?? id;

  return (
    <div style={{ display: 'grid', gap: 'var(--space-5)' }}>
      <section className="card">
        <h2 style={{ fontSize: 'var(--text-lg)', marginBottom: 'var(--space-3)' }}>Balances</h2>
        <div style={{ display: 'grid', gap: 'var(--space-2)' }}>
          {ledger.data.balances.map((b) => (
            <div key={b.userId} style={{ display: 'grid', gridTemplateColumns: '80px 1fr 110px', gap: 'var(--space-2)', alignItems: 'center' }}>
              <span>{name(b.userId)}</span>
              <div style={{ height: 10, borderRadius: 5, background: 'var(--color-surface-sunken)', overflow: 'hidden', display: 'flex', justifyContent: b.net < 0 ? 'flex-end' : 'flex-start' }}>
                <div
                  style={{
                    width: `${(Math.abs(b.net) / maxAbs) * 100}%`,
                    background: b.net >= 0 ? 'var(--color-ok)' : 'var(--color-unreasonable)',
                  }}
                />
              </div>
              <span style={{ textAlign: 'right', fontVariantNumeric: 'tabular-nums', color: b.net >= 0 ? 'var(--color-ok)' : 'var(--color-unreasonable)' }}>
                {b.net >= 0 ? '+' : ''}
                {formatMoney(b.net, base)}
              </span>
            </div>
          ))}
        </div>
      </section>

      <section className="card">
        <h2 style={{ fontSize: 'var(--text-lg)', marginBottom: 'var(--space-3)' }}>Settle up</h2>
        {ledger.data.suggestedTransfers.length === 0 && <p className="muted">All square!</p>}
        {ledger.data.suggestedTransfers.map((t, i) => (
          <p key={i}>
            <strong>{name(t.fromUser)}</strong> pays <strong>{name(t.toUser)}</strong>{' '}
            <span style={{ fontVariantNumeric: 'tabular-nums' }}>{formatMoney(t.amount, base)}</span>
          </p>
        ))}
        {ledger.data.settlements.length > 0 && (
          <p className="muted" style={{ marginTop: 'var(--space-2)' }}>
            Already settled:{' '}
            {ledger.data.settlements.map((s) => `${name(s.fromUser)} → ${name(s.toUser)} ${formatMoney(s.amount, base)}`).join(', ')}
          </p>
        )}
      </section>

      <section style={{ display: 'grid', gap: 'var(--space-3)' }}>
        <h2 style={{ fontSize: 'var(--text-lg)' }}>Expenses</h2>
        {ledger.data.expenses.map((e) => (
          <div key={e.id} className="card">
            <div style={{ display: 'flex', gap: 'var(--space-2)', alignItems: 'baseline', flexWrap: 'wrap' }}>
              <strong>{formatMoney(e.amount, e.currency)}</strong>
              {e.currency !== base && <span className="muted">≈ {formatMoney(e.amount * e.fxRateToBase, base)}</span>}
              <span className="badge">{e.category}</span>
              <span className="muted" style={{ flex: 1, textAlign: 'right' }}>
                paid by {name(e.paidBy)}
              </span>
            </div>
            <p className="muted" style={{ marginTop: 'var(--space-1)' }}>{e.note}</p>
          </div>
        ))}
      </section>
    </div>
  );
}
