import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { StrictMode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { App } from "../App";
import { LogsView } from "../pages/LogsView";
import { useSyncStore } from "../store";
import { syncPresentation } from "../syncPresentation";
import { runSyncCommand } from "../syncCommands";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));

const config = {
  bucket_name: "test-bucket", local_path: "/test", s3_prefix: null, aws_region: "us-east-1",
  sync_interval_secs: 60, exclusion_patterns: [], use_iam_role: true,
  notifications: { sync_complete_enabled: true, conflict_enabled: true, error_enabled: true },
};

describe("UI state and recovery", () => {
  beforeEach(() => {
    useSyncStore.getState().resetStore();
    vi.mocked(invoke).mockReset().mockResolvedValue(null);
  });

  it("does not report ready before configuration or while stopped", () => {
    const label = () => syncPresentation(useSyncStore.getState()).text;
    expect(label()).toBe("Não configurado");
    useSyncStore.getState().setConfig(config);
    expect(label()).toBe("Estado indisponível");
    useSyncStore.getState().setSyncRunning(false);
    useSyncStore.getState().updateFileStatus("sample.txt", { type: "Pending" });
    expect(label()).toBe("Parado");
    useSyncStore.getState().setSyncAction("starting");
    expect(label()).toBe("Conectando…");
    useSyncStore.getState().setSyncAction(null);
    useSyncStore.getState().setSyncRunning(true);
    expect(label()).toBe("Sincronizando");
    useSyncStore.getState().setOnline(false);
    expect(label()).toBe("Sem conexão");
    useSyncStore.getState().setOnline(true);
    useSyncStore.getState().updateFileStatus("sample.txt", { type: "Synced" });
    expect(label()).toBe("Atualizado");
    useSyncStore.getState().setCredentialAlert("Credenciais inválidas");
    expect(label()).toBe("Verifique as credenciais");
  });

  it("shows active sync files in the status bar across screens", async () => {
    await act(async () => { render(<App />); });
    const footer = within(screen.getByRole("contentinfo"));
    const { updateFileStatus, setTransferProgress } = useSyncStore.getState();
    const download = "nested/folder/download.txt";
    const firstUpload = "folder/first.txt";
    const secondUpload = "folder/second.txt";

    act(() => updateFileStatus(download, { type: "Syncing" }));
    expect(footer.getByText("download.txt")).toBeInTheDocument();

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Logs & Diagnóstico" }));
    });
    act(() => setTransferProgress({ path: download, operation: "Download", bytes_transferred: 0, total_bytes: 0, percent: 15 }));
    expect(footer.getByText("Baixando")).toBeInTheDocument();
    expect(footer.getByText("download.txt")).toBeInTheDocument();
    expect(footer.queryByText("15%")).not.toBeInTheDocument();

    act(() => {
      updateFileStatus(firstUpload, { type: "Syncing" });
      setTransferProgress({ path: firstUpload, operation: "Upload", bytes_transferred: 42, total_bytes: 100, percent: 42 });
      updateFileStatus(secondUpload, { type: "Syncing" });
      setTransferProgress({ path: secondUpload, operation: "Upload", bytes_transferred: 0, total_bytes: 100, percent: 0 });
    });
    expect(footer.getByText("+2")).toBeInTheDocument();
    act(() => updateFileStatus(download, { type: "Synced" }));
    expect(footer.getByText("Enviando")).toBeInTheDocument();
    expect(footer.getByText("first.txt")).toBeInTheDocument();
    expect(footer.getByText("42%")).toBeInTheDocument();
    expect(footer.getByText("+1")).toBeInTheDocument();

    act(() => updateFileStatus(firstUpload, { type: "Synced" }));
    expect(footer.getByText("second.txt")).toBeInTheDocument();
    act(() => updateFileStatus(secondUpload, { type: "Synced" }));
    expect(footer.queryByText("Enviando")).not.toBeInTheDocument();
    expect(footer.queryByText("second.txt")).not.toBeInTheDocument();
  });

  it("tracks pending commands and clears credential warnings only after successful start", async () => {
    useSyncStore.getState().setConfig(config);
    useSyncStore.getState().setCredentialAlert("Credenciais inválidas");
    let finish!: () => void;
    vi.mocked(invoke).mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
    const start = runSyncCommand("start_sync");
    expect(useSyncStore.getState().syncAction).toBe("starting");
    finish();
    await start;
    expect(useSyncStore.getState().syncRunning).toBe(true);
    expect(useSyncStore.getState().credentialAlert).toBeNull();
    await runSyncCommand("stop_sync");
    expect(syncPresentation(useSyncStore.getState()).text).toBe("Parado");
  });

  it("keeps logs visible when refresh fails and allows retry", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(["[INFO] sample"]);
    render(<LogsView />);
    await screen.findByText("[INFO] sample");
    vi.mocked(invoke).mockRejectedValueOnce(new Error("failure"));
    fireEvent.click(screen.getByRole("button", { name: "Atualizar" }));
    await screen.findByRole("alert");
    expect(screen.getByText("[INFO] sample")).toBeInTheDocument();
    vi.mocked(invoke).mockResolvedValueOnce(["[INFO] recovered"]);
    fireEvent.click(screen.getByRole("button", { name: "Tentar novamente" }));
    await screen.findByText("[INFO] recovered");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("does not present a failed log load as an empty history", async () => {
    vi.mocked(invoke).mockRejectedValue(new Error("failure"));
    render(<LogsView />);
    await screen.findByRole("alert");
    expect(screen.queryByText("Nenhum registro de log encontrado.")).not.toBeInTheDocument();
  });

  it("shows folder errors and retries opening the folder", async () => {
    useSyncStore.getState().setConfig(config);
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "open_local_folder") throw new Error("failure");
      if (command === "get_sync_running") return false;
      return null;
    });
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Abrir no Explorer" }));
    await screen.findByText("Não foi possível abrir a pasta local.");
    vi.mocked(invoke).mockResolvedValue(null);
    fireEvent.click(screen.getByRole("button", { name: "Tentar novamente" }));
    await waitFor(() => expect(screen.queryByText("Não foi possível abrir a pasta local.")).not.toBeInTheDocument());
  });
});

describe("Startup and draft navigation", () => {
  beforeEach(() => {
    useSyncStore.getState().resetStore();
    vi.mocked(invoke).mockReset();
  });

  it("opens synchronization after loading a saved configuration", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => command === "get_config" ? config : command === "get_sync_running" ? false : []);
    render(<StrictMode><App /></StrictMode>);
    await screen.findByRole("heading", { name: "Objetos & Transferências" });
    expect(screen.queryByRole("heading", { name: "Preferências do Sistema" })).not.toBeInTheDocument();
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_sync"));
    expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "start_sync")).toHaveLength(1);
  });

  it("does not restart an already running engine", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => command === "get_config" ? config : command === "get_sync_running" ? true : []);
    render(<App />);
    await waitFor(() => expect(useSyncStore.getState().syncRunning).toBe(true));
    expect(invoke).not.toHaveBeenCalledWith("start_sync");
  });

  it("shows startup progress, reports failure and retries only on request", async () => {
    let rejectStart!: (error: Error) => void;
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_config") return config;
      if (command === "get_sync_running") return false;
      if (command === "start_sync") return new Promise((_, reject) => { rejectStart = reject; });
      return [];
    });
    render(<App />);
    await screen.findByText("Conectando e verificando arquivos…");
    await act(async () => { rejectStart(new Error("Bucket inacessível")); });
    await screen.findByText(/Não foi possível iniciar a sincronização:.*Bucket inacessível/);
    expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "start_sync")).toHaveLength(1);
    vi.mocked(invoke).mockImplementation(async (command) => command === "get_sync_running" ? false : []);
    fireEvent.click(screen.getByRole("button", { name: "Tentar novamente" }));
    await waitFor(() => expect(useSyncStore.getState().syncRunning).toBe(true));
    expect(screen.queryByText(/Não foi possível iniciar a sincronização:/)).not.toBeInTheDocument();
  });

  it("opens the setup guide for an unconfigured app", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => command === "get_config" ? { ...config, bucket_name: "", local_path: "" } : []);
    render(<App />);
    await screen.findByRole("list", { name: "Etapas da configuração inicial" });
    expect(screen.queryByRole("heading", { name: "Objetos & Transferências" })).not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("start_sync");
  });

  it("keeps unsaved edits when switching between screens", async () => {
    useSyncStore.getState().setConfig(config);
    vi.mocked(invoke).mockResolvedValue([]);
    await act(async () => { render(<App />); });
    fireEvent.click(screen.getByRole("button", { name: "Configurações" }));
    fireEvent.change(screen.getByLabelText("Nome do Bucket S3"), { target: { value: "draft-bucket" } });
    fireEvent.click(screen.getByRole("button", { name: "Logs & Diagnóstico" }));
    fireEvent.click(screen.getByRole("button", { name: "Configurações" }));
    expect(screen.getByLabelText("Nome do Bucket S3")).toHaveValue("draft-bucket");
    expect(screen.getByText("Alterações não salvas")).toBeVisible();
    expect(useSyncStore.getState().config).toEqual(config);
  });
});
