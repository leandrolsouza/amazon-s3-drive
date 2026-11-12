import { fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { expect, it, vi } from "vitest";
import { StorageCostForecast } from "../components/StorageCostForecast";
import { S3MonthlySpend } from "../components/S3MonthlySpend";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

it("charges downloads only above the remaining allowance and adds storage", async () => {
  vi.mocked(invoke).mockRejectedValueOnce(new Error("Offline"));
  render(<StorageCostForecast storageUsd={0.5} region="us-east-1" />);
  await screen.findByText(/tarifa única editável/);
  fireEvent.change(screen.getByLabelText(/Downloads pela internet/), { target: { value: "150" } });
  fireEvent.change(screen.getByLabelText(/Franquia ainda disponível/), { target: { value: "100" } });
  expect(screen.getByText(/4,50/)).toBeInTheDocument();
  expect(screen.getByText(/5,00/)).toBeInTheDocument();

  fireEvent.change(screen.getByLabelText(/Tarifa de saída/), { target: { value: "" } });
  expect(screen.getByText("Informe a tarifa")).toBeInTheDocument();
});

it("uses regional catalog tiers for storage and outbound transfer", async () => {
  vi.mocked(invoke).mockResolvedValueOnce({
    storage: [{ start_gb: 0, end_gb: null, usd_per_gb: 0.02 }],
    transfer: [
      { start_gb: 0, end_gb: 10, usd_per_gb: 0.10 },
      { start_gb: 10, end_gb: null, usd_per_gb: 0.05 },
    ],
  });
  render(<StorageCostForecast storageUsd={99} storageBytes={10 * 1024 ** 3} region="sa-east-1" />);
  await screen.findByText(/catálogo público AWS/);
  fireEvent.change(screen.getByLabelText(/Downloads pela internet/), { target: { value: "20" } });
  expect(screen.getByText(/1,50/)).toBeInTheDocument();
  expect(screen.getByText(/1,70/)).toBeInTheDocument();
});

it("queries billing only on request and labels an incomplete response as a subtotal", async () => {
  vi.mocked(invoke).mockClear().mockResolvedValueOnce({
    fetched_at: "2026-01-10T00:00:00Z",
    start: "2026-01-01",
    end_exclusive: "2026-01-11",
    estimated: true,
    partial: true,
    rows: [{ usage_type: "DataTransfer-Out-Bytes", usd: 2.5 }],
  });
  render(<S3MonthlySpend />);
  expect(invoke).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Consultar gasto do mês" }));
  expect(await screen.findByText("Subtotal da primeira página")).toBeInTheDocument();
  expect(vi.mocked(invoke).mock.calls).toEqual([["get_s3_monthly_spend"]]);
});
