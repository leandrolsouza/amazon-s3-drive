import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, expect, it, vi } from "vitest";
import { SiteView } from "../pages/SiteView";
import { useSyncStore } from "../store";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

afterEach(() => {
  useSyncStore.getState().resetStore();
  localStorage.clear();
  vi.resetAllMocks();
});

const link = "https://example.cloudfront.net/";
const site = {
  id: "arn:aws:cloudformation:sa-east-1:000000000000:stack/S3DriveSite-pagina/example",
  bucket: "pagina",
  url: link,
};
const index = { key: "index.html", size: 100, revision: "index" };
const asset = { key: "assets/style.css", size: 50, revision: "css" };
const document = { key: "interno.pdf", size: 500, revision: "pdf" };

function configure() {
  useSyncStore.getState().setConfig({
    aws_region: "sa-east-1",
    bucket_name: "sync-example",
    local_path: "C:\\Sync",
    s3_prefix: "web",
    sync_interval_secs: 60,
    exclusion_patterns: [],
    notifications: { sync_complete_enabled: true, conflict_enabled: true, error_enabled: true },
    use_iam_role: false,
  });
}

it("suggests the bucket from the local folder and publishes only reviewed files", async () => {
  configure();
  const folder = "C:\\Sites\\Página";
  vi.mocked(invoke)
    .mockResolvedValueOnce([])
    .mockResolvedValueOnce([index, asset, document])
    .mockResolvedValueOnce(site)
    .mockRejectedValueOnce("Falha no envio.");

  render(<SiteView active />);
  fireEvent.change(screen.getByLabelText("Pasta do site"), { target: { value: folder } });
  expect(screen.getByLabelText("Nome do novo bucket")).toHaveValue("pagina");
  expect(screen.queryByRole("button", { name: "Publicar site" })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Revisar arquivos" }));
  fireEvent.click(await screen.findByRole("checkbox", { name: "interno.pdf" }));
  const publish = screen.getByRole("button", { name: "Publicar site" });
  await waitFor(() => expect(publish).toBeEnabled());
  fireEvent.click(publish);

  expect(await screen.findByLabelText("Link público")).toHaveValue(link);
  expect(invoke).toHaveBeenCalledWith("publish_site", {
    source: { kind: "local", folder },
    files: [index, asset],
    destination: { kind: "new", bucket: "pagina" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Republicar" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Falha no envio.");
  expect(screen.getByLabelText("Link público")).toHaveValue(link);
  expect(invoke).toHaveBeenLastCalledWith("publish_site", {
    source: { kind: "local", folder },
    files: [index, asset],
    destination: { kind: "existing", id: site.id },
  });
});

it("browses the configured S3 source and republishes into the selected site's bucket", async () => {
  configure();
  vi.mocked(invoke)
    .mockResolvedValueOnce([site])
    .mockResolvedValueOnce({ prefix: "", folders: ["site-clientes"], has_index: false })
    .mockResolvedValueOnce({ prefix: "site-clientes", folders: ["assets"], has_index: true })
    .mockResolvedValueOnce([index, asset])
    .mockResolvedValueOnce(site);

  render(<SiteView active />);
  await screen.findByRole("option", { name: "pagina" });
  fireEvent.change(screen.getByLabelText("Site"), { target: { value: site.id } });
  fireEvent.change(screen.getByLabelText("Origem dos arquivos"), { target: { value: "s3" } });
  fireEvent.click(await screen.findByRole("button", { name: "site-clientes/" }));
  await screen.findByText("s3://sync-example/web/site-clientes/");
  fireEvent.click(screen.getByRole("button", { name: "Revisar arquivos" }));
  fireEvent.click(await screen.findByRole("button", { name: "Republicar" }));
  await screen.findByText("Site publicado.");
  expect(invoke).toHaveBeenCalledWith("publish_site", {
    source: { kind: "s3", prefix: "site-clientes" },
    files: [index, asset],
    destination: { kind: "existing", id: site.id },
  });
});

it("restores a previous version after confirmation and retains the public link", async () => {
  configure();
  vi.mocked(invoke)
    .mockResolvedValueOnce([site])
    .mockResolvedValueOnce([
      { id: "active", timestamp_ms: null, active: true },
      { id: "previous", timestamp_ms: null, active: false },
    ])
    .mockResolvedValueOnce(site);

  render(<SiteView active />);
  await screen.findByRole("option", { name: "pagina" });
  fireEvent.change(screen.getByLabelText("Site"), { target: { value: site.id } });
  fireEvent.click(screen.getByRole("button", { name: "Versões" }));
  fireEvent.click(await screen.findByRole("button", { name: "Restaurar versão" }));
  expect(invoke).not.toHaveBeenCalledWith("restore_site_release", expect.anything());
  fireEvent.click(screen.getByRole("button", { name: "Confirmar restauração" }));
  await screen.findByText("Versão restaurada.");
  expect(screen.getByLabelText("Link público")).toHaveValue(link);
  expect(invoke).toHaveBeenCalledWith("restore_site_release", { id: site.id, release: "previous" });
});
