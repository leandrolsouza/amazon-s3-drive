import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { t, locale } from "../i18n";

interface MonthlySpend {
  fetched_at: string;
  start: string;
  end_exclusive: string;
  estimated: boolean;
  partial: boolean;
  rows: { usage_type: string; usd: number }[];
}

export function S3MonthlySpend() {
  const usd = new Intl.NumberFormat(locale, { style: "currency", currency: "USD", maximumFractionDigits: 4 });
  const [spend, setSpend] = useState<MonthlySpend | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function fetchSpend() {
    setBusy(true);
    setError(null);
    try {
      setSpend(await invoke<MonthlySpend>("get_s3_monthly_spend"));
    } catch (failure) {
      setError(typeof failure === "string" ? failure : t("Could not query Cost Explorer."));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="analytics-breakdown cost-forecast" aria-labelledby="monthly-spend-title">
      <h3 id="monthly-spend-title">{t("AWS spend this month")}</h3>
      <button type="button" className="btn-compact" onClick={fetchSpend} disabled={busy}>
        {busy ? t("Loading…") : t("Check this month's spend")}
      </button>
      <p>{t("Cost Explorer charges $0.01 per page. S3 charges may include other buckets and accounts accessible to these credentials.")}</p>
      {error && <p role="alert">{error}</p>}
      {spend && (
        <>
          <p>{t("Retrieved on") + " "}{new Date(spend.fetched_at).toLocaleString(locale)}{t(". UTC period from") + " "}{spend.start} {t("to") + " "}{spend.end_exclusive} {t("(exclusive).")}</p>
          <dl className="cost-forecast-totals" aria-live="polite">
            <div className="cost-forecast-total">
              <dt>{spend.partial ? t("First-page subtotal") : spend.estimated ? t("Recorded spend (AWS estimate)") : t("Recorded spend")}</dt>
              <dd>{usd.format(spend.rows.reduce((total, row) => total + row.usd, 0))}</dd>
            </div>
          </dl>
          {spend.rows.length > 0 && (
            <details className="cost-forecast-breakdown">
              <summary>{t("Break down by usage type")}</summary>
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
