import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { describe, it, expect, beforeEach, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { ConfigurationPage } from "../pages/ConfigurationPage";
import { useSyncStore } from "../store";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(null),
}));

describe("ConfigurationPage", () => {
  beforeEach(() => {
    useSyncStore.getState().resetStore();
    vi.mocked(invoke).mockReset().mockResolvedValue(null);
  });

  it("requires connection, folder and review before starting the first setup", async () => {
    render(<ConfigurationPage />);
    const startBtn = screen.getByRole("button", { name: "Salvar e iniciar" });
    expect(startBtn).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Continuar" }));
    await screen.findByText("Informe o bucket S3.");
    expect(screen.queryByRole("textbox", { name: "Pasta Local" })).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Nome do Bucket S3"), { target: { value: "test-bucket" } });
    fireEvent.click(screen.getByLabelText("Usar AWS IAM Role / Perfil de Instância"));
    fireEvent.click(screen.getByRole("button", { name: "Continuar" }));
    expect(await screen.findByRole("textbox", { name: "Pasta Local" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Continuar" }));
    expect(screen.getByText("Escolha a pasta local.")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Pasta Local"), { target: { value: "/test" } });
    fireEvent.click(screen.getByRole("button", { name: "Backup (Apenas Upload → S3)" }));
    fireEvent.click(screen.getByRole("button", { name: "Continuar" }));
    await screen.findByText("Revise antes de iniciar");
    expect(screen.getByText("Somente envio ao S3")).toBeInTheDocument();
    expect(startBtn).not.toBeDisabled();
    expect(invoke).not.toHaveBeenCalledWith("save_config", expect.anything());
    expect(invoke).not.toHaveBeenCalledWith("start_sync");
    fireEvent.click(screen.getByRole("button", { name: "Voltar" }));
    expect(screen.getByLabelText("Pasta Local")).toHaveValue("/test");
  });

  it("displays field-specific validation error messages when ValidationError[] is returned", () => {
    render(
      <ConfigurationPage
        initialValidationErrors={[
          {
            field: "access_key_id",
            reason:
              "Access Key ID deve conter exatamente 20 caracteres alfanuméricos maiúsculos.",
          },
          {
            field: "secret_access_key",
            reason: "Secret Access Key deve conter exatamente 40 caracteres.",
          },
        ]}
      />
    );

    expect(screen.getByTestId("error-access_key_id")).toHaveTextContent(
      /20 caracteres/i
    );
    expect(screen.getByTestId("error-secret_access_key")).toHaveTextContent(
      /40 caracteres/i
    );
  });

  it("keeps error notifications toggle always enabled and disabled in UI (Requirement 10.4)", () => {
    render(<ConfigurationPage />);
    const errorToggle = screen.getByLabelText(
      /Notificar erros de sincronização/i
    );
    expect(errorToggle).toBeChecked();
    expect(errorToggle).toBeDisabled();
  });
});

const configured = {
  bucket_name: "test-bucket", local_path: "/test", s3_prefix: null, aws_region: "us-east-1",
  sync_interval_secs: 60, exclusion_patterns: [], use_iam_role: true,
  notifications: { sync_complete_enabled: true, conflict_enabled: true, error_enabled: true },
};

describe("Explicit configuration saving", () => {
  beforeEach(() => {
    useSyncStore.getState().resetStore();
    useSyncStore.getState().setConfig(configured);
    vi.mocked(invoke).mockReset().mockResolvedValue(null);
  });

  it("keeps notification and bucket edits pending until an explicit save", async () => {
    render(<ConfigurationPage />);
    fireEvent.change(screen.getByLabelText("Nome do Bucket S3"), { target: { value: "changed-bucket" } });
    fireEvent.click(screen.getByText("Ajustes avançados de sincronização"));
    fireEvent.click(screen.getByLabelText("Notificar ao detectar conflitos de arquivos"));
    expect(screen.getByText("Alterações não salvas")).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalled();
    expect(useSyncStore.getState().config).toEqual(configured);
    expect(screen.getByRole("button", { name: "Testar Conexão" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Salvar alterações" }));
    await screen.findByText("Alterações salvas.");
    expect(useSyncStore.getState().config?.bucket_name).toBe("changed-bucket");
    expect(useSyncStore.getState().config?.notifications.conflict_enabled).toBe(false);
    expect(screen.getByText("Todas as alterações estão salvas")).toBeInTheDocument();
  });

  it("retains unsaved changes after a save failure and lets the user discard", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "save_config") throw new Error("Unable to save");
      return [];
    });
    render(<ConfigurationPage />);
    fireEvent.change(screen.getByLabelText("Nome do Bucket S3"), { target: { value: "changed-bucket" } });
    fireEvent.click(screen.getByRole("button", { name: "Salvar alterações" }));
    await screen.findByText(/Unable to save/);
    expect(screen.getByText("Alterações não salvas")).toBeInTheDocument();
    expect(useSyncStore.getState().config).toEqual(configured);
    fireEvent.click(screen.getByRole("button", { name: "Descartar alterações" }));
    expect(screen.getByLabelText("Nome do Bucket S3")).toHaveValue("test-bucket");
    expect(screen.getByText("Todas as alterações estão salvas")).toBeInTheDocument();
  });

  it("imports a profile as a draft and applies it only on save", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => command === "import_config_profile" ? { ...configured, bucket_name: "imported-bucket" } : []);
    render(<ConfigurationPage />);
    fireEvent.click(screen.getByRole("button", { name: "Importar JSON" }));
    await screen.findByText(/Perfil carregado para revisão/);
    expect(screen.getByLabelText("Nome do Bucket S3")).toHaveValue("imported-bucket");
    expect(useSyncStore.getState().config).toEqual(configured);
    expect(invoke).not.toHaveBeenCalledWith("save_config", expect.anything());
    fireEvent.click(screen.getByRole("button", { name: "Salvar alterações" }));
    await waitFor(() => expect(useSyncStore.getState().config?.bucket_name).toBe("imported-bucket"));
  });
});
