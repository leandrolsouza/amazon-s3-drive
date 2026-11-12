import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { DeletionReviewPanel } from "../components/DeletionReviewPanel";
import { DeletionReview } from "../types";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);
const review: DeletionReview = {
  s3_key: "file.txt", local_path: "/drive/file.txt", deleted_side: "local",
  other_side_changed: false, remote_etag: "version", local_hash: null,
};
let pending: DeletionReview[];

describe("Deletion review", () => {
  beforeEach(() => {
    pending = [review];
    invokeMock.mockReset().mockImplementation(async (command) => {
      if (command === "get_deletion_reviews") return pending;
      return null;
    });
  });
  it("loads persisted reviews and restores only after user choice", async () => {
    render(<DeletionReviewPanel />);
    await screen.findByRole("region", { name: "Revisar exclusões" });
    expect(invokeMock).not.toHaveBeenCalledWith("resolve_deletion", expect.anything());
    fireEvent.click(screen.getByRole("button", { name: "Restaurar arquivo" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("resolve_deletion", { review, restore: true }));
    await waitFor(() => expect(screen.queryByRole("region")).not.toBeInTheDocument());
  });
  it("excludes conflicts from the batch deletion choice", async () => {
    pending = [review, { ...review, s3_key: "edited.txt", other_side_changed: true }];
    render(<DeletionReviewPanel />);
    await screen.findByText(/Conflito: a outra versão foi editada/);
    fireEvent.click(screen.getByRole("button", { name: "Confirmar exclusões sem conflito" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("resolve_deletion", { review, restore: false }));
    expect(invokeMock.mock.calls.filter(([cmd]) => cmd === "resolve_deletion")).toHaveLength(1);
    expect(screen.getByText(/edited.txt/)).toBeInTheDocument();
  });
  it("keeps failed decisions visible and disables actions during submission", async () => {
    let fail!: (reason: Error) => void;
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_deletion_reviews") return pending;
      return new Promise((_, reject) => { fail = reject; });
    });
    render(<DeletionReviewPanel />);
    await screen.findByRole("region");
    fireEvent.click(screen.getByRole("button", { name: "Confirmar exclusão" }));
    expect(screen.getByRole("button", { name: "Restaurar arquivo" })).toBeDisabled();
    await act(async () => fail(new Error("Versão mudou")));
    expect(screen.getByRole("alert")).toHaveTextContent("Versão mudou");
    expect(screen.getByRole("button", { name: "Confirmar exclusão" })).toBeEnabled();
  });
});
