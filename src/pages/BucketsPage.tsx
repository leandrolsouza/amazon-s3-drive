import { FormEvent, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { DeletionReviewPanel } from "../components/DeletionReviewPanel";
import type { AppConfig, ConflictEvent, FileStatus, GlobalSyncState } from "../types";
import { t } from "../i18n";

interface BucketSummary {
  id: number;
  bucket_name: string;
  aws_region: string;
  local_path: string;
  s3_prefix: string | null;
  enabled: boolean;
  running: boolean;
  busy: boolean;
  state: GlobalSyncState;
  error: string | null;
}

interface BucketDetails {
  files: FileStatus[];
  logs: string[];
  conflicts: ConflictEvent[];
}

type DiscoveredBucket = [name: string, region: string | null];

const bucketStatus: Record<GlobalSyncState, string> = {
  UpToDate: "Up to date",
  Syncing: "Syncing",
  Error: "Errors",
  Offline: "Offline",
};

const fileStatus: Record<FileStatus["state"]["type"], string> = {
  Pending: "Pending",
  Syncing: "Syncing",
  Synced: "Synced",
  Error: "Error",
};

function BucketActivity({ bucket }: { bucket: BucketSummary }) {
  const [details, setDetails] = useState<BucketDetails | null>(null);
  const [loadError, setLoadError] = useState("");
  const [actionError, setActionError] = useState("");
  const [busy, setBusy] = useState(false);
  const [revision, setRevision] = useState(0);
  const { id } = bucket;

  useEffect(() => {
    let active = true;
    let pending = false;
    const refresh = async () => {
      if (pending) return;
      pending = true;
      try {
        const result = await invoke<BucketDetails>("get_bucket_details", { id });
        if (active) {
          setDetails(result);
          setLoadError("");
        }
      } catch (error) {
        if (active) setLoadError(String(error));
      } finally {
        pending = false;
      }
    };
    void refresh();
    const timer = setInterval(refresh, 3000);
    return () => { active = false; clearInterval(timer); };
  }, [id, revision]);

  async function resolveConflict(path: string, strategy: string) {
    setBusy(true);
    setActionError("");
    try {
      await invoke("resolve_bucket_conflict", { id, path, strategy });
      setRevision((value) => value + 1);
    } catch (error) {
      setActionError(String(error));
    } finally {
      setBusy(false);
    }
  }

  return <div className="bucket-activity">
    <DeletionReviewPanel bucketId={id} paused={!bucket.running || bucket.busy} />
    {loadError && <p role="alert">{loadError}</p>}
    {actionError && <p role="alert">{actionError}</p>}
    {details?.conflicts.map((conflict) => <div className="banner banner-warning" key={conflict.conflict_copy_path}>
      <strong>{t("Conflict in") + " "}{conflict.file_name}</strong>
      <div className="bucket-actions">
        {[["both", t("Keep both")], ["local", t("Keep local")], ["remote", t("Keep remote")]].map(([strategy, label]) => (
          <button key={strategy} type="button" className="btn-compact" disabled={busy || bucket.busy}
            onClick={() => void resolveConflict(conflict.conflict_copy_path, strategy)}>{label}</button>
        ))}
      </div>
    </div>)}
    {!details && !loadError && <p role="status">{t("Loading activity…")}</p>}
    {details && <>
      <h4>{t("Files (")}{details.files.length})</h4>
      {details.files.length === 0 ? <p>{t("No files in the sync monitor.")}</p> : (
        <ul className="bucket-files">
          {details.files.map((file) => <li key={file.path}>
            <span className="mono">{file.path}</span>
            <span>{file.state.type === "Error" ? file.state.message : t(fileStatus[file.state.type])}</span>
          </li>)}
        </ul>
      )}
      {details.logs.length > 0 && <details>
        <summary>{t("Recent logs")}</summary>
        <pre className="bucket-logs">{details.logs.join("\n")}</pre>
      </details>}
    </>}
  </div>;
}

export function BucketsPage({ config, primaryStatus, onOpenPrimary, onOpenConfig }: {
  config: AppConfig | null;
  primaryStatus: string;
  onOpenPrimary: () => void;
  onOpenConfig: () => void;
}) {
  const [buckets, setBuckets] = useState<BucketSummary[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [loadError, setLoadError] = useState("");
  const [revision, setRevision] = useState(0);
  const [selected, setSelected] = useState<number | null>(null);
  const [changing, setChanging] = useState<Partial<Record<number, boolean>>>({});
  const [bucketErrors, setBucketErrors] = useState<Partial<Record<number, string>>>({});
  const [discovered, setDiscovered] = useState<DiscoveredBucket[] | null>(null);
  const [name, setName] = useState("");
  const [region, setRegion] = useState(config?.aws_region || "us-east-1");
  const [path, setPath] = useState("");
  const [prefix, setPrefix] = useState("");
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState("");
  const [formError, setFormError] = useState("");
  const [mode, setMode] = useState<"existing" | "create">("existing");

  useEffect(() => {
    let active = true;
    let pending = false;
    const refresh = async () => {
      if (pending) return;
      pending = true;
      try {
        const result = await invoke<BucketSummary[]>("list_sync_buckets");
        if (active) {
          setBuckets(result);
          setLoaded(true);
          setLoadError("");
        }
      } catch (error) {
        if (active) setLoadError(String(error));
      } finally {
        pending = false;
      }
    };
    void refresh();
    const timer = setInterval(refresh, 2500);
    return () => { active = false; clearInterval(timer); };
  }, [revision]);

  async function runFormAction(action: () => Promise<void>) {
    setBusy(true);
    setFormError("");
    setNotice("");
    try {
      await action();
    } catch (error) {
      setFormError(String(error));
    } finally {
      setBusy(false);
    }
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const bucketName = name.trim();
    const awsRegion = region.trim();
    void runFormAction(async () => {
      if (mode === "create") {
        await invoke("create_s3_bucket", { bucketName, awsRegion });
        setMode("existing");
        setNotice(t("Bucket {0} was created. Choose a folder to set up sync.", bucketName));
        return;
      }
      const id = await invoke<number>("add_sync_bucket", {
        input: { bucket_name: bucketName, aws_region: awsRegion, local_path: path.trim(), s3_prefix: prefix.trim() || null },
      });
      setSelected(id);
      setRevision((value) => value + 1);
      setNotice(t("Bucket {0} was added. Select Start to begin syncing.", bucketName));
      setName("");
      setPath("");
      setPrefix("");
    });
  }

  async function changeSync(bucket: BucketSummary, enabled: boolean) {
    setChanging((current) => ({ ...current, [bucket.id]: true }));
    setBucketErrors((current) => ({ ...current, [bucket.id]: undefined }));
    try {
      await invoke("set_bucket_sync_enabled", { id: bucket.id, enabled });
    } catch (error) {
      setBucketErrors((current) => ({ ...current, [bucket.id]: String(error) }));
    } finally {
      setRevision((value) => value + 1);
      setChanging((current) => ({ ...current, [bucket.id]: false }));
    }
  }

  return <div className="pane-container">
    <header className="command-bar storage-header"><h2 className="pane-title">Buckets</h2></header>
    <div className="storage-content bucket-page">
      <section aria-label={t("Configured buckets")}>
        {config?.bucket_name && <article className="settings-group bucket-card">
          <div className="bucket-card-heading">
            <div>
              <h3>{config.bucket_name}</h3>
              <p>{t("Primary ·") + " "}{config.aws_region} · {primaryStatus}</p>
              <p className="mono">{config.local_path}</p>
            </div>
            <button type="button" className="btn-compact" onClick={onOpenPrimary}>{t("Open status")}</button>
          </div>
        </article>}
        {loadError && <p role="alert">{t("Could not load buckets.") + " "}{loadError}</p>}
        {!loaded && !loadError && <p role="status">{t("Loading buckets…")}</p>}
        {buckets.map((bucket) => {
          const isChanging = Boolean(changing[bucket.id]) || bucket.busy;
          const error = bucketErrors[bucket.id] || bucket.error;
          const status = isChanging ? t("Refreshing…") : error ? t("Error")
            : bucket.running ? t(bucketStatus[bucket.state]) : bucket.enabled ? t("Waiting to start") : t("Paused");
          return <article className="settings-group bucket-card" key={bucket.id}>
            <div className="bucket-card-heading">
              <div>
                <h3>{bucket.bucket_name}</h3>
                <p>{bucket.aws_region} · {status}{bucket.s3_prefix && ` · ${bucket.s3_prefix}`}</p>
                <p className="mono">{bucket.local_path}</p>
              </div>
              <div className="bucket-actions">
                <button type="button" className="btn-compact" aria-expanded={selected === bucket.id}
                  onClick={() => setSelected(selected === bucket.id ? null : bucket.id)}>{t("Activity")}</button>
                <button type="button" className="btn-primary-compact" disabled={isChanging}
                  onClick={() => void changeSync(bucket, !bucket.running)}>
                  {bucket.running ? t("Pause") : error ? t("Retry start") : t("Start")}
                </button>
                {bucket.enabled && !bucket.running && !isChanging && <button type="button" className="btn-compact"
                  onClick={() => void changeSync(bucket, false)}>{t("Pause")}</button>}
              </div>
            </div>
            {error && <p className="field-error" role="alert">{error}</p>}
            {selected === bucket.id && <BucketActivity bucket={bucket} />}
          </article>;
        })}
      </section>
      <section className="settings-group bucket-card" aria-labelledby="bucket-form-title">
        <div className="bucket-card-heading">
          <h3 id="bucket-form-title">{mode === "create" ? t("Create bucket on AWS") : t("Add existing bucket")}</h3>
          <button type="button" className="btn-compact" disabled={busy || (mode === "existing" && Boolean(config?.endpoint_url?.trim()))}
            onClick={() => { setMode(mode === "existing" ? "create" : "existing"); setNotice(""); setFormError(""); }}>
            {mode === "existing" ? t("Create a bucket") : t("Use an existing bucket")}
          </button>
        </div>
        {!config?.bucket_name && <p>{t("Set up your AWS connection to use your buckets.")}{" "}
          <button type="button" className="btn-compact" onClick={onOpenConfig}>{t("Open settings")}</button>
        </p>}
        <form className="site-form" onSubmit={submit}>
          <fieldset className="bucket-fields" disabled={busy || !config?.bucket_name}>
            {mode === "existing" && <div className="bucket-actions">
              <button type="button" className="btn-compact" onClick={() => void runFormAction(async () => {
                setDiscovered(await invoke<DiscoveredBucket[]>("discover_s3_buckets"));
              })}>{t("Find buckets in account")}</button>
              {discovered && (discovered.length > 0 ? <select aria-label={t("Buckets found")} value=""
                onChange={(event) => {
                  const entry = discovered.find(([bucketName]) => bucketName === event.target.value);
                  if (entry) { setName(entry[0]); if (entry[1]) setRegion(entry[1]); }
                }}>
                <option value="">{t("Select bucket")}</option>
                {discovered.map(([bucketName, bucketRegion]) => (
                  <option key={bucketName} value={bucketName}>{bucketName}{bucketRegion && ` · ${bucketRegion}`}</option>
                ))}
              </select> : <span>{t("No buckets found.")}</span>)}
            </div>}
            <label>{t("Bucket name")}<input type="text" value={name} onChange={(event) => setName(event.target.value)} required /></label>
            <label>{t("Region")}<input type="text" value={region} onChange={(event) => setRegion(event.target.value)} required placeholder="sa-east-1" /></label>
            {mode === "existing" ? <>
              <div className="bucket-folder-field">
                <label htmlFor="bucket-folder">{t("Local folder")}</label>
                <span className="control-inline">
                  <input id="bucket-folder" type="text" value={path} onChange={(event) => setPath(event.target.value)} required />
                  <button type="button" className="btn-compact" onClick={() => void runFormAction(async () => {
                    const folder = await invoke<string | null>("open_path_dialog");
                    if (folder) setPath(folder);
                  })}>{t("Choose…")}</button>
                </span>
              </div>
              <label>{t("S3 prefix (optional)")}<input type="text" value={prefix} onChange={(event) => setPrefix(event.target.value)} /></label>
            </> : <p>{t("The new bucket will be private with S3 Block Public Access enabled.")}</p>}
            <button type="submit" className="btn-primary-compact">
              {busy ? t("Please wait…") : mode === "create" ? t("Create bucket") : t("Add bucket")}
            </button>
          </fieldset>
        </form>
        {notice && <p role="status">{notice}</p>}
        {formError && <p className="field-error" role="alert">{formError}</p>}
      </section>
    </div>
  </div>;
}
