import { render, screen, act, fireEvent, waitFor, within } from "@testing-library/react";
import { describe, it, expect, beforeEach, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { StatusView } from "../pages/StatusView";
import { useSyncStore } from "../store";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue([]),
}));

describe("StatusView", () => {
  beforeEach(() => {
    useSyncStore.getState().resetStore();
    vi.mocked(invoke).mockReset().mockResolvedValue([]);
  });

  it("paginates folders and files while keeping batch selection across the result", () => {
    useSyncStore.getState().setFileStatuses([
      ...Array.from({ length: 99 }, (_, index) => ({
        path: `folder-${index}/nested.txt`, state: { type: "Synced" as const }, last_modified: null, size_bytes: 1,
      })),
      ...["alpha.txt", "beta.txt"].map((path) => ({
        path, state: { type: "Synced" as const }, last_modified: null, size_bytes: 1,
      })),
    ]);
    render(<StatusView />);
    expect(screen.getByText("Itens 1–100 de 101")).toBeInTheDocument();
    expect(screen.getByTestId("file-item-alpha.txt")).toBeInTheDocument();
    expect(screen.queryByTestId("file-item-beta.txt")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("checkbox", { name: "Selecionar todos os arquivos do resultado" }));
    fireEvent.click(screen.getByRole("button", { name: "Próxima página" }));
    expect(screen.getByText("Itens 101–101 de 101")).toBeInTheDocument();
    expect(within(screen.getByTestId("file-item-beta.txt")).getByRole("checkbox")).toBeChecked();
    expect(screen.queryByTestId("file-item-alpha.txt")).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "Filtrar arquivos" }), { target: { value: "alpha" } });
    expect(screen.getByTestId("file-item-alpha.txt")).toBeInTheDocument();
    expect(screen.queryByRole("navigation", { name: "Páginas da lista de arquivos" })).not.toBeInTheDocument();
  });

  it("keeps a trash item after a destination collision and removes it after a successful retry", async () => {
    useSyncStore.getState().setConfig({
      bucket_name: "test-bucket", local_path: "/test", s3_prefix: "workspace",
      aws_region: "us-east-1", sync_interval_secs: 60, exclusion_patterns: [], use_iam_role: true,
      notifications: { sync_complete_enabled: true, conflict_enabled: true, error_enabled: true },
    });
    const locator = "workspace/.s3-drive-trash/1700000000000000000/docs/deleted.txt";
    const restore = vi.fn()
      .mockResolvedValueOnce({ status: "blocked", local: true, remote: false })
      .mockResolvedValueOnce({ status: "restored", backup_retained: false });
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "list_trash") return {
        entries: [{
          source: "remote", locator, original_key: "workspace/docs/deleted.txt",
          file_name: "deleted.txt", deleted_at: "2023-11-14T22:13:20Z", size_bytes: 32,
        }],
        local_error: null,
        remote_error: null,
      };
      if (command === "restore_trash") return restore(args);
      return [];
    });
    render(<StatusView />);
    fireEvent.click(screen.getByRole("button", { name: "Lixeira" }));
    const trash = within(screen.getByRole("dialog", { name: "Lixeira" }));
    expect(await trash.findByText("workspace/docs/deleted.txt")).toBeInTheDocument();
    fireEvent.click(trash.getByRole("button", { name: "Restaurar" }));
    expect(await trash.findByRole("alert")).toHaveTextContent("Já existe um arquivo no destino neste computador.");
    expect(restore).toHaveBeenCalledWith({ bucketId: undefined, source: "remote", locator, legacyPath: undefined });
    fireEvent.click(trash.getByRole("button", { name: "Restaurar" }));
    expect(await trash.findByText("Arquivo restaurado na pasta local e no S3.")).toBeInTheDocument();
    expect(trash.queryByText("workspace/docs/deleted.txt")).not.toBeInTheDocument();
  });

  it("filters attention across folders and counts each S3 key once", async () => {
    useSyncStore.getState().setConfig({
      bucket_name: "test-bucket", local_path: "/test", s3_prefix: "workspace",
      aws_region: "us-east-1", sync_interval_secs: 60, exclusion_patterns: [], use_iam_role: true,
      notifications: { sync_complete_enabled: true, conflict_enabled: false, error_enabled: true },
    });
    useSyncStore.getState().addConflict({
      s3_key: "workspace/docs/conflict.txt", file_name: "conflict.txt",
      local_path: "/test/docs/conflict.txt", conflict_copy_path: "/test/docs/conflict-copy.txt",
      local_modified: "", remote_modified: "", local_size_bytes: 1, remote_size_bytes: 1,
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_last_file_sync_time") return "2026-01-01T12:00:00Z";
      if (command === "get_sync_status") return [
        { path: "workspace/errors/failed.txt", state: { type: "Error", message: "Timeout" }, last_modified: null, size_bytes: 1 },
        { path: "workspace/docs/conflict.txt", state: { type: "Synced" }, last_modified: null, size_bytes: 1 },
        { path: "workspace/normal.txt", state: { type: "Pending" }, last_modified: null, size_bytes: 1 },
      ];
      return [];
    });
    const reviews = ["workspace/errors/failed.txt", "workspace/deleted.txt"].map((s3_key) => ({
      s3_key, local_path: "/test/file.txt", deleted_side: "local" as const,
      other_side_changed: false, local_hash: null, remote_etag: null,
    }));
    render(<StatusView reviews={reviews} />);

    const summary = within(screen.getByRole("region", { name: "Resumo de atividade do bucket" }));
    expect(summary.getByText("s3://test-bucket/workspace")).toBeInTheDocument();
    await summary.findByText(/Última sincronização de arquivo registrada:.*2026/);
    const attention = await summary.findByRole("button", { name: "Precisa de atenção (3)" });
    fireEvent.click(attention);

    expect(attention).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByTestId("file-item-workspace/errors/failed.txt")).toBeInTheDocument();
    expect(screen.getByTestId("file-item-workspace/docs/conflict.txt")).toBeInTheDocument();
    expect(screen.getByTestId("notification-conflict-conflict.txt")).toBeInTheDocument();
    expect(screen.queryByTestId("file-item-workspace/normal.txt")).not.toBeInTheDocument();
  });

  it("requests an S3 check without restarting active sync", async () => {
    useSyncStore.getState().setSyncRunning(true);
    render(<StatusView />);
    fireEvent.click(screen.getByRole("button", { name: "Sincronizar agora" }));
    await screen.findByText("Verificação do S3 solicitada.");
    expect(invoke).toHaveBeenCalledWith("sync_now");
    expect(invoke).not.toHaveBeenCalledWith("start_sync");
  });

  it.each(["docs", "nova/subpasta"])("reviews files and sends only after confirmation to %s under the configured prefix", async (destination) => {
    useSyncStore.getState().setConfig({
      bucket_name: "test-bucket", local_path: "/test", s3_prefix: "workspace",
      aws_region: "us-east-1", sync_interval_secs: 60, exclusion_patterns: [], use_iam_role: true,
      notifications: { sync_complete_enabled: true, conflict_enabled: true, error_enabled: true },
    });
    useSyncStore.getState().setFileStatuses([
      { path: "workspace/docs/existing.txt", state: { type: "Synced" }, last_modified: null, size_bytes: 1 },
    ]);
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "open_files_dialog") return ["/source/new.txt"];
      if (command === "import_files_to_drive") return 1;
      return [];
    });

    render(<StatusView />);
    fireEvent.click(screen.getByRole("button", { name: "Enviar arquivos" }));
    const dialog = screen.getByRole("dialog", { name: "Enviar arquivos" });
    expect(invoke).not.toHaveBeenCalledWith("open_files_dialog");
    const folder = within(dialog).getByLabelText("Pasta de destino");
    const newFolder = destination !== "docs";
    fireEvent.change(folder, { target: { value: newFolder ? within(dialog).getByRole<HTMLOptionElement>("option", { name: "Nova pasta…" }).value : destination } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Selecionar arquivos" }));
    await within(dialog).findByText("new.txt");
    const confirm = within(dialog).getByRole("button", { name: "Confirmar envio" });
    if (newFolder) {
      const path = within(dialog).getByLabelText("Caminho da nova pasta");
      expect(confirm).toBeDisabled();
      fireEvent.change(path, { target: { value: "../fora" } });
      expect(confirm).toBeDisabled();
      fireEvent.change(path, { target: { value: destination } });
    }
    expect(within(dialog).getByText(`s3://test-bucket/workspace/${destination}`)).toBeInTheDocument();
    expect(vi.mocked(invoke).mock.calls.some(([command]) => command === "import_files_to_drive")).toBe(false);
    fireEvent.click(confirm);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("import_files_to_drive", {
      paths: ["/source/new.txt"],
      targetSubfolder: destination,
    }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("renders the correct icon for each SyncState", () => {
    act(() => {
      useSyncStore.getState().setFileStatuses([
        {
          path: "pending.txt",
          state: { type: "Pending" },
          last_modified: null,
          size_bytes: 100,
        },
        {
          path: "syncing.txt",
          state: { type: "Syncing" },
          last_modified: null,
          size_bytes: 200,
        },
        {
          path: "synced.txt",
          state: { type: "Synced" },
          last_modified: null,
          size_bytes: 300,
        },
        {
          path: "error.txt",
          state: { type: "Error", message: "Timeout" },
          last_modified: null,
          size_bytes: 400,
        },
      ]);
    });

    render(<StatusView />);

    expect(screen.getByTestId("icon-Pending")).toHaveTextContent("⏳");
    expect(screen.getByTestId("icon-Syncing")).toHaveTextContent("🔄");
    expect(screen.getByTestId("icon-Synced")).toHaveTextContent("✅");
    expect(screen.getByTestId("icon-Error")).toHaveTextContent("❌");
  });

  it("includes pending and syncing files in the active filter", () => {
    useSyncStore.getState().setFileStatuses([
      { path: "pending.txt", state: { type: "Pending" }, last_modified: null, size_bytes: 10 },
      { path: "syncing.txt", state: { type: "Syncing" }, last_modified: null, size_bytes: 20 },
      { path: "complete.txt", state: { type: "Synced" }, last_modified: null, size_bytes: 30 },
    ]);
    render(<StatusView />);
    fireEvent.click(screen.getByRole("button", { name: "Ativos (2)" }));
    expect(screen.getByTestId("file-item-pending.txt")).toBeInTheDocument();
    expect(screen.getByTestId("file-item-syncing.txt")).toBeInTheDocument();
    expect(screen.queryByTestId("file-item-complete.txt")).not.toBeInTheDocument();
  });

  it("global state badge reflects priority Error > Syncing > UpToDate", () => {
    useSyncStore.getState().setConfig({ bucket_name: "test-bucket", local_path: "/test", s3_prefix: null, aws_region: "us-east-1", sync_interval_secs: 60, exclusion_patterns: [], use_iam_role: true, notifications: { sync_complete_enabled: true, conflict_enabled: true, error_enabled: true } });
    useSyncStore.getState().setSyncRunning(true);
    render(<StatusView />);
    const badge = screen.getByTestId("global-state-badge");
    expect(badge).toHaveTextContent(/Atualizado/i);

    act(() => {
      useSyncStore.getState().updateFileStatus("a.txt", { type: "Synced" });
      useSyncStore.getState().updateFileStatus("b.txt", { type: "Syncing" });
    });
    expect(screen.getByTestId("global-state-badge")).toHaveTextContent(
      /Sincronizando/i
    );

    act(() => {
      useSyncStore
        .getState()
        .updateFileStatus("c.txt", { type: "Error", message: "S3 500" });
    });
    expect(screen.getByTestId("global-state-badge")).toHaveTextContent(/Erro/i);
  });
});

 describe("High priority sync actions", () => {
  beforeEach(() => {
    useSyncStore.getState().resetStore();
    vi.mocked(invoke).mockReset().mockResolvedValue([]);
    useSyncStore.getState().setFileStatuses([
      { path: "first.txt", state: { type: "Synced" }, last_modified: null, size_bytes: 10 },
      { path: "second.txt", state: { type: "Synced" }, last_modified: null, size_bytes: 20 },
    ]);
  });

  it("requires confirmation for single deletion and preserves files on cancel", async () => {
    render(<StatusView />);
    fireEvent.click(screen.getByTestId("file-item-first.txt"));
    fireEvent.click(screen.getByRole("button", { name: "Excluir Objeto (Local + S3)" }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText("first.txt")).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("delete_object_sync", expect.anything());
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancelar" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(useSyncStore.getState().fileStatuses).toHaveLength(2);
    fireEvent.click(screen.getByRole("button", { name: "Excluir Objeto (Local + S3)" }));
    fireEvent.click(screen.getByRole("button", { name: "Excluir da pasta local e do S3" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("delete_object_sync", { s3Key: "first.txt" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(useSyncStore.getState().fileStatuses.map((file) => file.path)).toEqual(["second.txt"]);
  });

  it("confirms batch deletion and retains selection after a failed attempt", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "batch_delete_objects") throw new Error("failure");
      return [];
    });
    render(<StatusView />);
    fireEvent.click(screen.getByRole("checkbox", { name: "Selecionar todos os arquivos visíveis" }));
    fireEvent.click(screen.getByRole("button", { name: "Excluir Lote (2)" }));
    expect(invoke).not.toHaveBeenCalledWith("batch_delete_objects", expect.anything());
    fireEvent.click(screen.getByRole("button", { name: "Excluir da pasta local e do S3" }));
    await screen.findByText(/Não foi possível concluir a exclusão/);
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(useSyncStore.getState().fileStatuses).toHaveLength(2);
    vi.mocked(invoke).mockResolvedValue(2);
    fireEvent.click(screen.getByRole("button", { name: "Excluir da pasta local e do S3" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(invoke).toHaveBeenLastCalledWith("batch_delete_objects", { s3Keys: ["first.txt", "second.txt"] });
    expect(useSyncStore.getState().fileStatuses).toHaveLength(0);
  });

  it("shows start and stop errors instead of claiming success", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "start_sync" || command === "stop_sync") throw new Error("failure");
      if (command === "get_sync_running") return false;
      return [];
    });
    render(<StatusView />);
    fireEvent.click(screen.getByRole("button", { name: "Iniciar" }));
    await screen.findByText(/Não foi possível iniciar a sincronização/);
    expect(useSyncStore.getState().syncRunning).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Parar" }));
    await screen.findByText(/Não foi possível parar a sincronização/);
    expect(useSyncStore.getState().syncAction).toBeNull();
  });
});
