import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

const usd = new Intl.NumberFormat("pt-BR", {
  style: "currency",
  currency: "USD",
  maximumFractionDigits: 4,
});

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
      <h3 id="cost-forecast-title">Previsão mensal de custos</h3>
      <div className="cost-forecast-inputs">
        <label>
          Downloads pela internet (GB/mês)
          <input type="number" min="0" step="any" value={downloads} onChange={(event) => setDownloads(event.target.value)} />
        </label>
        <label>
          Franquia ainda disponível (GB)
          <input type="number" min="0" max="100" step="any" value={allowance} onChange={(event) => setAllowance(event.target.value)} />
        </label>
        <label>
          Tarifa de saída (US$/GB){region ? ` · ${region}` : ""}
          <input type="number" min="0" step="any" value={rate} onChange={(event) => setManualRate(event.target.value)} />
        </label>
      </div>
      {loadingPrices && <p role="status">Consultando tarifas da AWS…</p>}
      {!loadingPrices && (
        <p>
          {useTransferTiers ? "Saída calculada por faixas do catálogo público AWS para a região do bucket."
            : "Saída calculada com uma tarifa única editável."}
          {storageTiers.length > 0 ? " Armazenamento calculado com a tarifa AWS da classe configurada."
            : " Armazenamento usa a estimativa local de referência."}
          {manualRate !== null && firstRate !== undefined && (
            <> <button type="button" className="row-act-btn" onClick={() => setManualRate(null)}>Usar tarifa AWS</button></>
          )}
        </p>
      )}
      <p>A franquia de até 100 GB/mês é compartilhada com outros serviços e regiões elegíveis da AWS. Informe quanto dela ainda está disponível.</p>
      <dl className="cost-forecast-totals" aria-live="polite">
        <div><dt>Armazenamento</dt><dd>{usd.format(monthlyStorage)}</dd></div>
        <div><dt>Entrada pela internet</dt><dd>{usd.format(0)}</dd></div>
        <div>
          <dt>Saída pela internet</dt>
          <dd>{transferUsd === null ? "Informe a tarifa" : usd.format(transferUsd)}</dd>
        </div>
        <div className="cost-forecast-total"><dt>Total estimado / mês</dt><dd>{totalUsd === null ? "—" : usd.format(totalUsd)}</dd></div>
      </dl>
      <p>A simulação usa o volume rastreado e a classe configurada. Requisições, recuperação de dados, aceleração, mínimos por objeto e impostos ficam fora da conta.</p>
    </section>
  );
}
