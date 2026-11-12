import { render, screen, fireEvent, act } from "@testing-library/react";
import { describe, it, expect, beforeEach, vi } from "vitest";
import { NotificationHandler } from "../components/NotificationHandler";
import { useSyncStore } from "../store";

const invokeMock = vi.fn().mockResolvedValue(null);
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

describe("NotificationHandler", () => {
  beforeEach(() => {
    useSyncStore.getState().resetStore();
    invokeMock.mockClear();
  });

  it("does not display completion notification when sync_complete_enabled = false", () => {
    act(() => {
      useSyncStore.getState().setConfig({
        bucket_name: "b",
        local_path: "/p",
        s3_prefix: null,
        aws_region: "us-east-1",
        sync_interval_secs: 60,
        exclusion_patterns: [],
        notifications: {
          sync_complete_enabled: false,
          conflict_enabled: true,
          error_enabled: true,
        },
        use_iam_role: false,
      });
      useSyncStore.getState().setInitialCompleteCount(42);
    });

    render(<NotificationHandler />);
    expect(
      screen.queryByTestId("notification-sync-complete")
    ).not.toBeInTheDocument();
  });

  it("always displays error notification after 3 failed attempts regardless of settings", () => {
    act(() => {
      useSyncStore.getState().setConfig({
        bucket_name: "b",
        local_path: "/p",
        s3_prefix: null,
        aws_region: "us-east-1",
        sync_interval_secs: 60,
        exclusion_patterns: [],
        notifications: {
          sync_complete_enabled: false,
          conflict_enabled: false,
          error_enabled: false,
        },
        use_iam_role: false,
      });
      useSyncStore.getState().addError({
        file: "relatorio.pdf",
        error: "Max retries exceeded",
        retry_count: 3,
      });
    });

    render(<NotificationHandler />);
    expect(
      screen.getByTestId("notification-error-relatorio.pdf")
    ).toBeInTheDocument();
  });

  it("displays conflict notification with the 3 resolution options and invokes resolve_conflict", async () => {
    act(() => {
      useSyncStore.getState().addConflict({
        file_name: "orcamento.xlsx",
        local_path: "/drive/orcamento.xlsx",
        conflict_copy_path:
          "/drive/orcamento(conflito-2024-01-15-14-30-00).xlsx",
        local_modified: "2024-01-15 14:29:00 UTC",
        remote_modified: "2024-01-15 14:30:00 UTC",
        local_size_bytes: 1024,
        remote_size_bytes: 2048,
      });
    });

    render(<NotificationHandler />);

    const card = screen.getByTestId("notification-conflict-orcamento.xlsx");
    expect(card).toBeInTheDocument();

    const btnLocal = screen.getByRole("button", { name: /Manter local/i });
    const btnRemote = screen.getByRole("button", { name: /Manter remota/i });
    const btnBoth = screen.getByRole("button", { name: /Manter ambas/i });

    expect(btnLocal).toBeInTheDocument();
    expect(btnRemote).toBeInTheDocument();
    expect(btnBoth).toBeInTheDocument();

    await act(async () => {
      fireEvent.click(btnLocal);
    });

    expect(invokeMock).toHaveBeenCalledWith("resolve_conflict", {
      strategy: "local",
      localPath: "/drive/orcamento.xlsx",
      conflictPath: "/drive/orcamento(conflito-2024-01-15-14-30-00).xlsx",
    });
  });
});

describe("Conflict recovery", () => {
  beforeEach(() => {
    useSyncStore.getState().resetStore();
    invokeMock.mockReset().mockResolvedValue(null);
    useSyncStore.getState().addConflict({ file_name: "sample.txt", local_path: "/test/sample.txt", conflict_copy_path: "/test/sample-conflict.txt", local_modified: "2026-01-01", remote_modified: "2026-01-02", local_size_bytes: 10, remote_size_bytes: 20 });
  });

  it("keeps the conflict and allows retry after failure", async () => {
    invokeMock.mockRejectedValueOnce(new Error("failure"));
    render(<NotificationHandler />);
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Manter local" })); });
    expect(screen.getByTestId("notification-conflict-sample.txt")).toBeInTheDocument();
    expect(screen.getByText(/Não foi possível resolver o conflito/)).toBeInTheDocument();
    expect(useSyncStore.getState().conflicts).toHaveLength(1);
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Manter local" })); });
    expect(useSyncStore.getState().conflicts).toHaveLength(0);
  });

  it("disables all resolution choices while a request is pending", async () => {
    let complete!: () => void;
    invokeMock.mockImplementationOnce(() => new Promise<void>((resolve) => { complete = resolve; }));
    render(<NotificationHandler />);
    fireEvent.click(screen.getByRole("button", { name: "Manter local" }));
    expect(screen.getByRole("button", { name: "Manter local" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Manter remota" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Manter ambas" })).toBeDisabled();
    await act(async () => { complete(); });
    expect(useSyncStore.getState().conflicts).toHaveLength(0);
  });
});
