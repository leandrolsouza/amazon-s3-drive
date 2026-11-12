import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface MonthlySpend {
  fetched_at: string;
  start: string;
  end_exclusive: string;
  estimated: boolean;
  partial: boolean;
  rows: { usage_type: string; usd: number }[];
}

const usd = new Intl.NumberFormat("pt-BR", { style: "currency", currency: "USD", maximumFractionDigits: 4 });

export function S3MonthlySpend() {
  const [spend, setSpend] = useState<MonthlySpend | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function fetchSpend() {
    setBusy(true);
    setError(null);
    try {
      setSpend(await invoke<MonthlySpend>("get_s3_monthly_spend"));
    } catch (failure) {
      setError(typeof failure === "string" ? failure : "Não foi possível consultar o Cost Explorer.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="analytics-breakdown cost-forecast" aria-labelledby="monthly-spend-title">
      <h3 id="monthly-spend-title">Gasto do mês na AWS</h3>
      <button type="button" className="btn-compact" onClick={fetchSpend} disabled={busy}>
        {busy ? "Consultando…" : "Consultar gasto do mês"}
      </button>
      <p>Consulta ao Cost Explorer de US$ 0,01 por página. Os valores de S3 podem incluir outros buckets e contas visíveis à credencial.</p>
      {error && <p role="alert">{error}</p>}
      {spend && (
        <>
          <p>Consultado em {new Date(spend.fetched_at).toLocaleString("pt-BR")}. Período UTC de {spend.start} até {spend.end_exclusive} (exclusivo).</p>
          <dl className="cost-forecast-totals" aria-live="polite">
            <div className="cost-forecast-total">
              <dt>{spend.partial ? "Subtotal da primeira página" : spend.estimated ? "Gasto registrado (estimado pela AWS)" : "Gasto registrado"}</dt>
              <dd>{usd.format(spend.rows.reduce((total, row) => total + row.usd, 0))}</dd>
            </div>
          </dl>
          {spend.rows.length > 0 && (
            <details className="cost-forecast-breakdown">
              <summary>Detalhar por tipo de uso</summary>
              <dl>
                {spend.rows.map((row) => (
                  <div key={row.usage_type}><dt>{row.usage_type}</dt><dd>{usd.format(row.usd)}</dd></div>
                ))}
              </dl>
            </details>
          )}
        </>
      )}
    </section>
  );
}
