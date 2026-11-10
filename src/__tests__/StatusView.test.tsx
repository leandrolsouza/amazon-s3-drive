import { render, screen, act } from "@testing-library/react";
import { describe, it, expect, beforeEach, vi } from "vitest";
import { StatusView } from "../pages/StatusView";
import { useSyncStore } from "../store";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue([]),
}));

describe("StatusView", () => {
  beforeEach(() => {
    useSyncStore.getState().resetStore();
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

  it("global state badge reflects priority Error > Syncing > UpToDate", () => {
    render(<StatusView />);
    const badge = screen.getByTestId("global-state-badge");
    expect(badge).toHaveTextContent(/UpToDate/i);

    act(() => {
      useSyncStore.getState().updateFileStatus("a.txt", { type: "Synced" });
      useSyncStore.getState().updateFileStatus("b.txt", { type: "Syncing" });
    });
    expect(screen.getByTestId("global-state-badge")).toHaveTextContent(
      /Syncing/i
    );

    act(() => {
      useSyncStore
        .getState()
        .updateFileStatus("c.txt", { type: "Error", message: "S3 500" });
    });
    expect(screen.getByTestId("global-state-badge")).toHaveTextContent(/Error/i);
  });
});
