import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { BucketsPage } from "../pages/BucketsPage";
import type { AppConfig } from "../types";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const invokeMock = vi.mocked(invoke);
const config: AppConfig = {
  bucket_name: "principal",
  aws_region: "sa-east-1",
  local_path: "/drive/principal",
  s3_prefix: null,
  sync_interval_secs: 60,
  exclusion_patterns: [],
  use_iam_role: true,
  notifications: { sync_complete_enabled: true, conflict_enabled: true, error_enabled: true },
};
const bucket = {
  id: 17, bucket_name: "arquivo", aws_region: "sa-east-1",
  local_path: "/drive/arquivo", s3_prefix: "documentos/",
  enabled: false, running: false, busy: false, state: "UpToDate", error: null,
};

describe("Buckets", () => {
  beforeEach(() => {
    invokeMock.mockReset().mockImplementation(async (command) => {
      if (command === "list_sync_buckets") return [bucket];
      if (command === "get_bucket_details") return { files: [], logs: [], conflicts: [] };
      if (command === "get_bucket_deletions") return [];
      if (command === "add_sync_bucket") return bucket.id;
      return null;
    });
  });

  function renderBuckets() {
    render(<BucketsPage config={config} primaryStatus="Em dia" onOpenPrimary={vi.fn()} onOpenConfig={vi.fn()} />);
  }

  it("creates a private bucket and then configures its local folder in a separate action", async () => {
    renderBuckets();
    fireEvent.click(screen.getByRole("button", { name: "Criar novo bucket" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Nome do bucket" }), { target: { value: "novo-bucket" } });
    fireEvent.click(screen.getByRole("button", { name: "Criar bucket" }));
    await screen.findByText(/Bucket novo-bucket criado/);
    expect(invokeMock).toHaveBeenCalledWith("create_s3_bucket", { bucketName: "novo-bucket", awsRegion: "sa-east-1" });
    expect(invokeMock.mock.calls.some(([command]) => command === "add_sync_bucket")).toBe(false);

    fireEvent.change(screen.getByRole("textbox", { name: "Pasta local" }), { target: { value: "/drive/novo" } });
    fireEvent.click(screen.getByRole("button", { name: "Adicionar bucket" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("add_sync_bucket", {
      input: { bucket_name: "novo-bucket", aws_region: "sa-east-1", local_path: "/drive/novo", s3_prefix: null },
    }));
    expect(invokeMock.mock.calls.some(([command]) => command === "set_bucket_sync_enabled")).toBe(false);
  });

  it("starts and loads activity by bucket ID", async () => {
    renderBuckets();
    const heading = await screen.findByRole("heading", { name: "arquivo" });
    const card = within(heading.closest("article")!);
    fireEvent.click(card.getByRole("button", { name: "Iniciar" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("set_bucket_sync_enabled", { id: 17, enabled: true }));
    fireEvent.click(card.getByRole("button", { name: "Atividade" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_bucket_details", { id: 17 }));
    expect(invokeMock).toHaveBeenCalledWith("get_bucket_deletions", { id: 17 });
  });
});
