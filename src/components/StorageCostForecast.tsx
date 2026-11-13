import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { t, locale } from "../i18n";

function nonNegative(value: string) {
  const number = Number(value);
  return Number.isFinite(number) && number >= 0 ? number : 0;
}

interface PriceTier {
  start_gb: number;
  end_gb: number | null;
  usd_per_gb: number;
}

interface PublicPrices {
  storage: PriceTier[];
  transfer: PriceTier[];
}

function tieredCost(volume: number, tiers: PriceTier[]) {
  return tiers.reduce((cost, tier) => {
    const inTier = Math.max(0, Math.min(volume, tier.end_gb ?? volume) - tier.start_gb);
    return cost + inTier * tier.usd_per_gb;
  }, 0);
}

interface Props {
  storageUsd: number;
  storageBytes?: number;
  region: string;
  customEndpoint?: boolean;
}

export function StorageCostForecast({ storageUsd, storageBytes = 0, region, customEndpoint }: Props) {
  const usd = new Intl.NumberFormat(locale, {
    style: "currency",
    currency: "USD",
    maximumFractionDigits: 4,
  });
  const [downloads, setDownloads] = useState("0");
  const [allowance, setAllowance] = useState("0");
  const [manualRate, setManualRate] = useState<string | null>(null);
  const [prices, setPrices] = useState<PublicPrices | null>(null);
  const [loadingPrices, setLoadingPrices] = useState(Boolean(region && !customEndpoint));

  useEffect(() => {
    if (!region || customEndpoint) return;
    let disposed = false;
    invoke<PublicPrices>("get_s3_public_prices")
      .then((result) => { if (!disposed && result) setPrices(result); })
      .catch(() => {})
      .finally(() => { if (!disposed) setLoadingPrices(false); });
    return () => { disposed = true; };
  }, [region, customEndpoint]);

  const transferTiers = prices?.transfer ?? [];
  const storageTiers = prices?.storage ?? [];
  const firstRate = transferTiers[0]?.usd_per_gb;
  const fallbackRate = !customEndpoint && region === "us-east-1" ? "0.09" : "";
  const rate = manualRate ?? firstRate?.toString() ?? fallbackRate;
  const useTransferTiers = manualRate === null && transferTiers.length > 0;
  const billableGb = Math.max(0, nonNegative(downloads) - Math.min(100, nonNegative(allowance)));
  const transferUsd = useTransferTiers ? tieredCost(billableGb, transferTiers)
    : billableGb === 0 || rate !== "" ? billableGb * nonNegative(rate) : null;
  const monthlyStorage = storageTiers.length > 0 ? tieredCost(storageBytes / 1024 ** 3, storageTiers) : storageUsd;
  const totalUsd = transferUsd === null ? null : monthlyStorage + transferUsd;

  return (
    <section className="analytics-breakdown cost-forecast" aria-labelledby="cost-forecast-title">
      <h3 id="cost-forecast-title">{t("Monthly cost forecast")}</h3>
      <div className="cost-forecast-inputs">
        <label>
          {t("Internet downloads (GB/month)")}
          <input type="number" min="0" step="any" value={downloads} onChange={(event) => setDownloads(event.target.value)} />
        </label>
        <label>
          {t("Remaining free allowance (GB)")}
          <input type="number" min="0" max="100" step="any" value={allowance} onChange={(event) => setAllowance(event.target.value)} />
        </label>
        <label>
          {t("Egress rate (USD/GB)")}{region ? ` · ${region}` : ""}
          <input type="number" min="0" step="any" value={rate} onChange={(event) => setManualRate(event.target.value)} />
        </label>
      </div>
      {loadingPrices && <p role="status">{t("Fetching AWS rates…")}</p>}
      {!loadingPrices && (
        <p>
          {useTransferTiers ? t("Egress uses tiered rates from the public AWS price list for the bucket's region.")
            : t("Egress uses a single editable rate.")}
          {storageTiers.length > 0 ? t(" Storage uses the AWS rate for the configured storage class.")
            : t(" Storage uses the local baseline estimate.")}
          {manualRate !== null && firstRate !== undefined && (
            <> <button type="button" className="row-act-btn" onClick={() => setManualRate(null)}>{t("Use AWS rate")}</button></>
          )}
        </p>
      )}
      <p>{t("The allowance of up to 100 GB per month is shared across eligible AWS services and regions. Enter how much remains available.")}</p>
      <dl className="cost-forecast-totals" aria-live="polite">
        <div><dt>{t("Storage")}</dt><dd>{usd.format(monthlyStorage)}</dd></div>
        <div><dt>{t("Internet ingress")}</dt><dd>{usd.format(0)}</dd></div>
        <div>
          <dt>{t("Internet egress")}</dt>
          <dd>{transferUsd === null ? t("Enter a rate") : usd.format(transferUsd)}</dd>
        </div>
        <div className="cost-forecast-total"><dt>{t("Estimated total / month")}</dt><dd>{totalUsd === null ? "—" : usd.format(totalUsd)}</dd></div>
      </dl>
      <p>{t("The forecast uses tracked volume and the configured storage class. It excludes requests, retrieval, acceleration, per-object minimums, and taxes.")}</p>
    </section>
  );
}
