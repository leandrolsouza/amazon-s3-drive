import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useSyncStore } from "../store";
import { formatBytes } from "./StatusView";
import { t, locale } from "../i18n";

type SiteSource = { kind: "local"; folder: string } | { kind: "s3"; prefix: string };
type SiteFile = { key: string; size: number; revision: string };
type PublishedSite = { id: string; bucket: string; url: string | null };
type SiteFolder = { prefix: string; folders: string[]; has_index: boolean };
type SiteRelease = { id: string; timestamp_ms: number | null; active: boolean };

const folderPreference = "s3drive-site-folder";

function savedFolder() {
  try {
    return localStorage.getItem(folderPreference) ?? "";
  } catch {
    return "";
  }
}

function validBucket(name: string) {
  return /^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$/.test(name)
    && !/^(xn--|sthree-|amzn-s3-demo-)/.test(name)
    && !/(-s3alias|--ol-s3|--x-s3|--table-s3|-an)$/.test(name);
}

function suggestedBucket(source: SiteSource, sourceBucket: string, sourcePrefix: string) {
  const path = source.kind === "local" ? source.folder : source.prefix || sourcePrefix;
  const segments = path.split(/[\\/]/).filter(Boolean);
  const name = segments[segments.length - 1] || sourceBucket;
  const normalized = name.normalize("NFD").replace(/[\u0300-\u036f]/g, "")
    .toLowerCase().replace(/[^a-z0-9]+/g, "-").slice(0, 63).replace(/^-+|-+$/g, "");
  if (validBucket(normalized) && normalized !== sourceBucket) return normalized;
  return normalized ? "site-" + normalized.slice(0, 54).replace(/-+$/g, "") + "-web" : t("my-site");
}

export function SiteView({ active }: { active: boolean }) {
  const config = useSyncStore((state) => state.config);
  const region = config?.aws_region.trim() ?? "";
  const customEndpoint = Boolean(config?.endpoint_url?.trim());
  const sourceBucket = config?.bucket_name.trim() ?? "";
  const sourcePrefix = config?.s3_prefix?.replace(/^\/+|\/+$/g, "") ?? "";
  if (!region || customEndpoint) {
    return <SitePane><p className="banner">{customEndpoint
      ? t("Publishing is available with a direct AWS connection.")
      : t("Set up your AWS connection to publish.")}</p></SitePane>;
  }
  return <SiteWorkspace
    key={JSON.stringify([region, sourceBucket, sourcePrefix, config?.use_iam_role])}
    active={active} sourceBucket={sourceBucket} sourcePrefix={sourcePrefix} region={region}
  />;
}

function SitePane({ children }: { children: React.ReactNode }) {
  return (
    <div className="pane-container">
      <header className="command-bar storage-header"><h2 className="pane-title">Sites</h2></header>
      <div className="storage-content">{children}</div>
    </div>
  );
}

function SiteWorkspace({ active, sourceBucket, sourcePrefix, region }: {
  active: boolean; sourceBucket: string; sourcePrefix: string; region: string;
}) {
  const [sites, setSites] = useState<PublishedSite[]>([]);
  const [siteId, setSiteId] = useState("");
  const [catalogError, setCatalogError] = useState("");
  const [catalogLoading, setCatalogLoading] = useState(false);
  const [catalogAttempt, setCatalogAttempt] = useState(0);
  const catalogRevision = useRef(0);
  const [origin, setOrigin] = useState<SiteSource["kind"]>("local");
  const [folder, setFolder] = useState(savedFolder);
  const [remoteFolder, setRemoteFolder] = useState<SiteFolder | null>(null);
  const [bucket, setBucket] = useState("");
  const [bucketEdited, setBucketEdited] = useState(false);
  const [preview, setPreview] = useState<{ source: SiteSource; files: SiteFile[] } | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [releases, setReleases] = useState<SiteRelease[] | null>(null);
  const [restoreChoice, setRestoreChoice] = useState<string | null>(null);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [feedback, setFeedback] = useState("");

  const site = sites.find((item) => item.id === siteId);
  const files = preview?.files.filter((file) => selected.has(file.key)) ?? [];
  const size = files.reduce((total, file) => total + file.size, 0);
  const validDestination = Boolean(site) || (validBucket(bucket.trim()) && bucket.trim() !== sourceBucket);

  useEffect(() => {
    if (!active) {
      setCatalogLoading(false);
      return;
    }
    let cancelled = false;
    const revision = catalogRevision.current;
    setCatalogLoading(true);
    setCatalogError("");
    invoke<PublishedSite[]>("list_published_sites")
      .then((result) => {
        if (cancelled || revision !== catalogRevision.current) return;
        setSites(result);
        setSiteId((current) => result.some((item) => item.id === current) ? current : "");
      })
      .catch((reason) => {
        if (!cancelled && revision === catalogRevision.current) setCatalogError(String(reason));
      })
      .finally(() => { if (!cancelled) setCatalogLoading(false); });
    return () => { cancelled = true; };
  }, [active, catalogAttempt]);

  function changeSite(id: string) {
    setSiteId(id);
    setReleases(null);
    setRestoreChoice(null);
    setError("");
    setFeedback("");
    if (!id) {
      setBucketEdited(false);
      const source: SiteSource = origin === "local" ? { kind: "local", folder } : { kind: "s3", prefix: remoteFolder?.prefix ?? "" };
      setBucket(origin === "s3" || folder.trim() ? suggestedBucket(source, sourceBucket, sourcePrefix) : "");
    }
  }

  function setSourceFolder(path: string) {
    setFolder(path);
    setPreview(null);
    if (!bucketEdited) setBucket(path.trim() ? suggestedBucket({ kind: "local", folder: path }, sourceBucket, sourcePrefix) : "");
  }

  async function run(message: string, action: () => Promise<void>) {
    setBusy(message);
    setError("");
    setFeedback("");
    try {
      await action();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy("");
    }
  }

  function browse(prefix: string) {
    setPreview(null);
    void run(t("Loading S3 folders…"), async () => {
      const result = await invoke<SiteFolder>("browse_site_source", { prefix });
      setRemoteFolder(result);
      if (!bucketEdited) setBucket(suggestedBucket({ kind: "s3", prefix }, sourceBucket, sourcePrefix));
    });
  }

  function changeOrigin(kind: SiteSource["kind"]) {
    setOrigin(kind);
    setPreview(null);
    setError("");
    if (kind === "s3") {
      browse(remoteFolder?.prefix ?? "");
    } else if (!bucketEdited) {
      setBucket(folder.trim() ? suggestedBucket({ kind: "local", folder }, sourceBucket, sourcePrefix) : "");
    }
  }

  function review() {
    const source: SiteSource = origin === "local"
      ? { kind: "local", folder }
      : { kind: "s3", prefix: remoteFolder?.prefix ?? "" };
    setPreview(null);
    void run(t("Preparing the file list…"), async () => {
      const result = await invoke<SiteFile[]>("preview_site_source", { source });
      setPreview({ source, files: result });
      setSelected(new Set(result.map((file) => file.key)));
      if (!bucketEdited) setBucket(suggestedBucket(source, sourceBucket, sourcePrefix));
    });
  }

  function toggleFile(key: string) {
    setSelected((current) => {
      const next = new Set(current);
      if (!next.delete(key)) next.add(key);
      return next;
    });
  }

  function saveSite(published: PublishedSite) {
    catalogRevision.current += 1;
    setSites((current) => [...current.filter((item) => item.id !== published.id), published]
      .sort((a, b) => a.bucket.localeCompare(b.bucket)));
    changeSite(published.id);
  }

  function publish(event: React.FormEvent) {
    event.preventDefault();
    if (busy || catalogLoading || !preview || !selected.has("index.html") || !validDestination) return;
    const destination = site ? { kind: "existing", id: site.id } : { kind: "new", bucket: bucket.trim() };
    void run(t("Publishing and waiting for CloudFront…"), async () => {
      const published = await invoke<PublishedSite>("publish_site", { source: preview.source, files, destination });
      saveSite(published);
      setFeedback(t("Site published."));
      if (preview.source.kind === "local") {
        try {
          localStorage.setItem(folderPreference, preview.source.folder);
        } catch {
          // The selected folder is still available for this session.
        }
      }
    });
  }

  function showHistory(id: string) {
    void run(t("Loading releases…"), async () => {
      setReleases(await invoke<SiteRelease[]>("list_site_releases", { id }));
      setRestoreChoice(null);
    });
  }

  function restore(id: string, release: string) {
    void run(t("Restoring the release in CloudFront…"), async () => {
      const published = await invoke<PublishedSite>("restore_site_release", { id, release });
      saveSite(published);
      setFeedback(t("Release restored."));
    });
  }

  async function copyLink(url: string) {
    try {
      await navigator.clipboard.writeText(url);
      setFeedback(t("Link copied."));
    } catch {
      setFeedback(t("Select and copy the address in the Public link field."));
    }
  }

  return (
    <SitePane>
      <form className="site-form" onSubmit={publish}>
        <label htmlFor="site-destination">Site</label>
        <div className="site-input-row">
          <select id="site-destination" value={siteId} disabled={Boolean(busy) || catalogLoading} onChange={(event) => changeSite(event.target.value)}>
            <option value="">{t("Create another site")}</option>
            {sites.map((item) => <option key={item.id} value={item.id}>{item.bucket}</option>)}
          </select>
          <button type="button" className="btn-compact" disabled={Boolean(busy) || catalogLoading} onClick={() => setCatalogAttempt((value) => value + 1)}>{t("Refresh sites")}</button>
        </div>
        {catalogLoading && <p role="status">{t("Loading sites from AWS…")}</p>}
        {catalogError && <p className="banner banner-error" role="alert">{catalogError}</p>}
        {site && (
          <>
            {site.url && <label htmlFor="site-url">{t("Public link")}</label>}
            <div className="site-input-row">
              {site.url && (
                <>
                  <input id="site-url" type="text" className="mono" readOnly value={site.url} onFocus={(event) => event.currentTarget.select()} />
                  <button type="button" className="btn-compact" onClick={() => { if (site.url) void copyLink(site.url); }}>{t("Copy link")}</button>
                </>
              )}
              <button type="button" className="btn-compact" disabled={Boolean(busy)} onClick={() => showHistory(site.id)}>{t("Releases")}</button>
            </div>
          </>
        )}
        {site && releases && (
          <section className="site-review" aria-label={t("Site releases")}>
            <h3>{t("Site releases")}</h3>
            {!releases.length && <p>{t("No releases found.")}</p>}
            <ul>
              {releases.map((release) => (
                <li key={release.id}>
                  <span>{release.timestamp_ms === null ? release.id.slice(0, 8) : new Date(release.timestamp_ms).toLocaleString(locale)}{release.active ? t(" · Active") : ""}</span>
                  {!release.active && (
                    restoreChoice === release.id ? (
                      <div className="site-input-row">
                        <button type="button" className="btn-compact" disabled={Boolean(busy)} onClick={() => restore(site.id, release.id)}>{t("Confirm restore")}</button>
                        <button type="button" className="btn-compact" disabled={Boolean(busy)} onClick={() => setRestoreChoice(null)}>{t("Cancel")}</button>
                      </div>
                    ) : (
                      <button type="button" className="btn-compact" disabled={Boolean(busy)} onClick={() => setRestoreChoice(release.id)}>{t("Restore release")}</button>
                    )
                  )}
                </li>
              ))}
            </ul>
          </section>
        )}

        <label htmlFor="site-origin">{t("File source")}</label>
        <select id="site-origin" value={origin} disabled={Boolean(busy)} onChange={(event) => changeOrigin(event.target.value === "s3" ? "s3" : "local")}>
          <option value="local">{t("Local folder")}</option>
          <option value="s3" disabled={!sourceBucket}>{t("Synced bucket")}</option>
        </select>
        {origin === "local" ? (
          <>
            <label htmlFor="site-folder">{t("Site folder")}</label>
            <div className="site-input-row">
              <input id="site-folder" type="text" value={folder} onChange={(event) => setSourceFolder(event.target.value)} disabled={Boolean(busy)} placeholder={t("Full folder path")} />
              {/Windows/i.test(navigator.userAgent) && (
                <button type="button" className="btn-compact" disabled={Boolean(busy)} onClick={() => void run(t("Waiting for folder selection…"), async () => {
                  const path = await invoke<string | null>("open_path_dialog");
                  if (path) setSourceFolder(path);
                })}>{t("Choose folder")}</button>
              )}
            </div>
          </>
        ) : (
          <section className="site-review" aria-label={t("Folders in the synced bucket")}>
            <p className="mono">s3://{[sourceBucket, sourcePrefix, remoteFolder?.prefix].filter(Boolean).join("/")}/</p>
            <div className="site-input-row">
              {remoteFolder?.prefix && (
                <button type="button" className="btn-compact" disabled={Boolean(busy)} onClick={() => browse(remoteFolder.prefix.split("/").slice(0, -1).join("/"))}>{t("Back to parent folder")}</button>
              )}
              {!remoteFolder && <button type="button" className="btn-compact" disabled={Boolean(busy)} onClick={() => browse("")}>{t("Load folders")}</button>}
            </div>
            {remoteFolder && !remoteFolder.has_index && <p>{t("Select a folder with index.html at its root.")}</p>}
            {remoteFolder && (
              <ul>{remoteFolder.folders.map((name) => (
                <li key={name}><button type="button" className="btn-compact" disabled={Boolean(busy)} onClick={() => browse([remoteFolder.prefix, name].filter(Boolean).join("/"))}>{name}/</button></li>
              ))}</ul>
            )}
          </section>
        )}

        {!site && (
          <>
            <label htmlFor="site-bucket">{t("New bucket name")}</label>
            <div className="site-input-row">
              <input id="site-bucket" type="text" value={bucket} maxLength={63} disabled={Boolean(busy)} onChange={(event) => { setBucketEdited(true); setBucket(event.target.value); }} />
              <button type="button" className="btn-compact" disabled={Boolean(busy) || !bucket.trim()} onClick={() => {
                const suffix = Math.random().toString(36).slice(2, 7).padEnd(5, "0");
                setBucketEdited(true);
                setBucket((value) => value.trim().slice(0, 57).replace(/-+$/g, "") + "-" + suffix);
              }}>{t("Suggest another name")}</button>
            </div>
            {bucket.trim() && !validDestination && <p>{t("Use an available name of 3 to 63 lowercase letters, numbers, or hyphens. Start and end with a letter or number.")}</p>}
          </>
        )}
        <div>
          <button type="button" className="btn-compact" disabled={Boolean(busy) || (origin === "local" ? !folder.trim() : !remoteFolder?.has_index)} onClick={review}>{t("Review files")}</button>
        </div>

        {preview && (
          <section className="site-review" aria-labelledby="site-review-heading">
            <h3 id="site-review-heading">{t("Files to publish")}</h3>
            <p>{files.length} {t("of") + " "}{preview.files.length} {t("files selected ·") + " "}{formatBytes(size)}</p>
            <p>{t("Destination in") + " "}{region} · <strong>{site?.bucket ?? (bucket.trim() || t("enter the bucket name"))}</strong></p>
            <ul>{preview.files.map((file) => (
              <li key={file.key}>
                <label><input type="checkbox" checked={selected.has(file.key)} disabled={Boolean(busy) || file.key === "index.html"} onChange={() => toggleFile(file.key)} /> {file.key}</label>
                <small>{formatBytes(file.size)}</small>
              </li>
            ))}</ul>
            <p>{t("Anyone with the link will be able to access the selected files. Publishing may incur S3 and CloudFront charges.")}</p>
            <div>
              <button type="submit" className="btn-primary-compact" disabled={Boolean(busy) || catalogLoading || !validDestination}>{site?.url ? t("Republish") : t("Publish site")}</button>
            </div>
          </section>
        )}
        {busy && <p role="status">{busy}</p>}
        {error && <p className="banner banner-error" role="alert">{error}</p>}
        {feedback && <p role="status">{feedback}</p>}
      </form>
    </SitePane>
  );
}
