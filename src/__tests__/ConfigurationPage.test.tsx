import { render, screen, fireEvent } from "@testing-library/react";
import { describe, it, expect, beforeEach, vi } from "vitest";
import { ConfigurationPage } from "../pages/ConfigurationPage";
import { useSyncStore } from "../store";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(null),
}));

describe("ConfigurationPage", () => {
  beforeEach(() => {
    useSyncStore.getState().resetStore();
  });

  it("disables 'Iniciar Sync' button when bucket_name or local_path are empty", () => {
    render(<ConfigurationPage />);

    const startBtn = screen.getByRole("button", { name: /Iniciar Sync/i });
    expect(startBtn).toBeDisabled();

    const bucketInput = screen.getByLabelText(/Nome do Bucket S3/i);
    fireEvent.change(bucketInput, { target: { value: "meu-bucket" } });
    expect(startBtn).toBeDisabled();

    const pathInput = screen.getByLabelText(/Pasta Local/i);
    fireEvent.change(pathInput, { target: { value: "/caminho/local" } });
    expect(startBtn).not.toBeDisabled();
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
