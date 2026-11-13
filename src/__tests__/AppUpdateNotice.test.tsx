import { fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, expect, it, vi } from "vitest";
import { AppUpdateNotice } from "../components/AppUpdateNotice";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const invokeMock = vi.mocked(invoke);

beforeEach(() => { invokeMock.mockReset(); });

it("checks on open and installs the announced version only after confirmation", async () => {
  invokeMock.mockResolvedValueOnce("0.2.0").mockRejectedValueOnce(new Error("Falha"));
  render(<AppUpdateNotice />);
  const button = await screen.findByRole("button", { name: "Atualizar e reiniciar" });
  expect(invokeMock.mock.calls).toEqual([["check_app_update"]]);
  expect(screen.getByRole("status")).toHaveTextContent("Versão 0.2.0 disponível.");

  fireEvent.click(button);
  expect(invokeMock).toHaveBeenCalledWith("install_app_update", { version: "0.2.0" });
  expect(await screen.findByRole("alert")).toHaveTextContent("Não foi possível atualizar");
  expect(button).toBeEnabled();
});
