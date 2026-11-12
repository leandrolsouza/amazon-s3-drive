import React, { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useSyncStore } from "../store";
import { runSyncCommand } from "../syncCommands";
import {
  AppConfig,
  AwsCredentials,
  ValidationError,
  validateCredentialsFormat,
} from "../types";

const DEFAULT_CONFIG: AppConfig = {
  bucket_name: "",
  local_path: "",
  s3_prefix: null,
  aws_region: "us-east-1",
  sync_interval_secs: 60,
  exclusion_patterns: [],
  notifications: {
    sync_complete_enabled: true,
    conflict_enabled: true,
    error_enabled: true,
  },
  use_iam_role: false,
  endpoint_url: null,
  force_path_style: false,
  max_concurrent_transfers: 4,
  storage_class: "STANDARD",
  server_side_encryption: false,
  bandwidth_limit_kbps: 0,
  launch_on_startup: false,
  sync_direction: "Bidirectional",
};

const QUICK_EXCLUSION_PRESETS = [
  "*.tmp",
  "*.log",
  ".DS_Store",
  "Thumbs.db",
  "~$*",
];

export interface ConfigurationPageProps {
  initialConfig?: AppConfig;
  initialValidationErrors?: ValidationError[];
  onSyncStarted?: () => void;
}

export const ConfigurationPage: React.FC<ConfigurationPageProps> = ({
  initialConfig,
  initialValidationErrors = [],
  onSyncStarted,
}) => {
  const storeConfig = useSyncStore((s) => s.config);
  const setStoreConfig = useSyncStore((s) => s.setConfig);
  const syncAction = useSyncStore((s) => s.syncAction);

  const [config, setConfig] = useState<AppConfig>(
    initialConfig ?? storeConfig ?? DEFAULT_CONFIG
  );
  const [savedConfig, setSavedConfig] = useState(initialConfig ?? storeConfig ?? DEFAULT_CONFIG);
  const [firstSetup, setFirstSetup] = useState(() => !(initialConfig ?? storeConfig)?.bucket_name.trim() || !(initialConfig ?? storeConfig)?.local_path.trim());
  const [setupStep, setSetupStep] = useState(0);
  const [isSaving, setIsSaving] = useState(false);
  const [isValidating, setIsValidating] = useState(false);
  const [credentialsSaved, setCredentialsSaved] = useState(false);
  const [creds, setCreds] = useState<AwsCredentials>({
    access_key_id: "",
    secret_access_key: "",
    session_token: "",
    region: config.aws_region || "us-east-1",
    use_iam_role: config.use_iam_role,
  });

  const [showSecretKey, setShowSecretKey] = useState(false);
  const [newPattern, setNewPattern] = useState("");
  const [validationErrors, setValidationErrors] = useState<ValidationError[]>(
    initialValidationErrors
  );
  const [statusMessage, setStatusMessage] = useState<string | null>(null);
  const [errorBanner, setErrorBanner] = useState<string | null>(null);
  const [isTestingConn, setIsTestingConn] = useState(false);

  useEffect(() => {
    if (!initialConfig && !storeConfig) {
      invoke<AppConfig | null>("get_config")
        .then((loaded) => {
          if (!loaded) return;
          setConfig(loaded);
          setSavedConfig(loaded);
          setFirstSetup(!loaded.bucket_name.trim() || !loaded.local_path.trim());
          setStoreConfig(loaded);
          setCreds((prev) => ({
            ...prev,
            region: loaded.aws_region || prev.region,
            use_iam_role: loaded.use_iam_role,
          }));
        })
        .catch(() => setErrorBanner("Não foi possível carregar a configuração. Reabra a tela para tentar novamente."));
    }
  }, [initialConfig, storeConfig, setStoreConfig]);

  const getFieldError = (field: string): string | undefined =>
    validationErrors.find((e) => e.field === field)?.reason;

  const canStartSync =
    config.bucket_name.trim().length > 0 && config.local_path.trim().length > 0;
  const credentialsEdited = !config.use_iam_role && Boolean(creds.access_key_id || creds.secret_access_key || creds.session_token);
  const isDirty = JSON.stringify(config) !== JSON.stringify(savedConfig) || credentialsEdited;
  const busy = isSaving || isTestingConn || isValidating || syncAction !== null;

  useEffect(() => {
    if (isDirty) setStatusMessage(null);
  }, [config, creds, isDirty]);

  const handleSelectPath = async () => {
    try {
      const selected = await invoke<string | null>("open_path_dialog");
      if (selected) {
        setConfig((prev) => ({ ...prev, local_path: selected }));
      }
    } catch (e) {
      setErrorBanner(String(e));
    }
  };

  const handleSaveConfig = async (cfgToSave: AppConfig = config) => {
    setErrorBanner(null);
    setStatusMessage(null);
    try {
      const errs = await invoke<ValidationError[]>("validate_config", {
        config: cfgToSave,
      });
      if (errs && errs.length > 0) {
        setValidationErrors(errs);
        setErrorBanner(errs.map((error) => error.reason).join(" "));
        return false;
      }
      setValidationErrors([]);
      await invoke("save_config", { config: cfgToSave });
      setStoreConfig(cfgToSave);
      setSavedConfig(cfgToSave);
      setStatusMessage("Configuração gravada.");
      return true;
    } catch (e) {
      setErrorBanner(String(e));
      return false;
    }
  };

  const handleSaveCredentials = async () => {
    setErrorBanner(null);
    setStatusMessage(null);
    const syncedCreds: AwsCredentials = {
      ...creds,
      region: config.aws_region,
      use_iam_role: config.use_iam_role,
      session_token: creds.session_token?.trim()
        ? creds.session_token.trim()
        : undefined,
    };

    const formatErrors = validateCredentialsFormat(syncedCreds);
    if (formatErrors.length > 0) {
      setValidationErrors(formatErrors);
      return false;
    }

    try {
      await invoke("save_credentials", { creds: syncedCreds });
      setCredentialsSaved(true);
      setValidationErrors([]);
      setCreds((previous) => ({ ...previous, access_key_id: "", secret_access_key: "", session_token: "" }));
      setStatusMessage("Credenciais gravadas no gerenciador seguro do sistema.");
      return true;
    } catch (e) {
      setErrorBanner(String(e));
      return false;
    }
  };

  const handleSaveAll = async () => {
    if (isSaving) return false;
    if (credentialsEdited) {
      const errors = validateCredentialsFormat({ ...creds, region: config.aws_region, use_iam_role: config.use_iam_role });
      if (errors.length) {
        setValidationErrors(errors);
        setErrorBanner("Revise os campos de autenticação antes de salvar.");
        if (firstSetup) setSetupStep(0);
        return false;
      }
    }
    setIsSaving(true);
    try {
      if (!await handleSaveConfig(config)) return false;
      if (credentialsEdited && !await handleSaveCredentials()) return false;
      setStatusMessage("Alterações salvas.");
      return true;
    } finally {
      setIsSaving(false);
    }
  };

  const handleTestConnection = async () => {
    if (isDirty || busy) return;
    setErrorBanner(null);
    setStatusMessage(null);
    setIsTestingConn(true);
    try {
      await invoke("test_connection");
      setStatusMessage("Acesso ao bucket S3 confirmado (HTTP 200).");
    } catch (e) {
      setErrorBanner(String(e));
    } finally {
      setIsTestingConn(false);
    }
  };

  const handleStartSync = async () => {
    if (!canStartSync || busy) return;
    setErrorBanner(null);
    setStatusMessage(null);

    const saved = await handleSaveAll();
    if (!saved) return;

    try {
      await runSyncCommand("start_sync");
      setStatusMessage(null);
      setFirstSetup(false);
      onSyncStarted?.();
    } catch (e) {
      setErrorBanner(String(e));
    }
  };

  const handleAddPatternValue = async (patternValue: string) => {
    const trimmed = patternValue.trim();
    if (!trimmed || config.exclusion_patterns.includes(trimmed)) return;
    const candidateConfig: AppConfig = {
      ...config,
      exclusion_patterns: [...config.exclusion_patterns, trimmed],
    };

    try {
      const errs = await invoke<ValidationError[]>("validate_config", {
        config: candidateConfig,
      });
      const globErr = errs?.find((e) => e.field === "exclusion_patterns");
      if (globErr) {
        setValidationErrors([globErr]);
        return;
      }
    } catch {
      if (trimmed.includes("[") && !trimmed.includes("]")) {
        setValidationErrors([
          {
            field: "exclusion_patterns",
            reason: `Padrão glob inválido '${trimmed}'`,
          },
        ]);
        return;
      }
    }

    setValidationErrors((prev) =>
      prev.filter((e) => e.field !== "exclusion_patterns")
    );
    setConfig(candidateConfig);
    setNewPattern("");
  };

  const handleAddPattern = () => handleAddPatternValue(newPattern);

  const handleRemovePattern = (pattern: string) => {
    setConfig((prev) => ({
      ...prev,
      exclusion_patterns: prev.exclusion_patterns.filter((p) => p !== pattern),
    }));
  };

  const handleNotificationToggle = (
    field: "sync_complete_enabled" | "conflict_enabled",
    checked: boolean
  ) => {
    const updated: AppConfig = {
      ...config,
      notifications: {
        ...config.notifications,
        [field]: checked,
        error_enabled: true,
      },
    };
    setConfig(updated);
  };

  const handleExportProfile = async () => {
    setErrorBanner(null);
    setStatusMessage(null);
    try {
      const savedPath = await invoke<string | null>("export_config_profile");
      if (savedPath) {
        setStatusMessage(`Perfil exportado com sucesso para: ${savedPath}`);
      }
    } catch (e) {
      setErrorBanner(String(e));
    }
  };

  const handleImportProfile = async () => {
    setErrorBanner(null);
    setStatusMessage(null);
    try {
      const imported = await invoke<AppConfig | null>("import_config_profile");
      if (imported) {
        setConfig(imported);
        setStatusMessage("Perfil carregado para revisão. Clique em Salvar alterações para aplicar.");
      }
    } catch (e) {
      setErrorBanner(String(e));
    }
  };

  const handleNextStep = async () => {
    if (busy) return;
    setErrorBanner(null);
    if (setupStep === 0) {
      const errors: ValidationError[] = [];
      if (!config.bucket_name.trim()) errors.push({ field: "bucket_name", reason: "Informe o bucket S3." });
      errors.push(...validateCredentialsFormat({ ...creds, region: config.aws_region, use_iam_role: config.use_iam_role || (credentialsSaved && !credentialsEdited) }));
      setValidationErrors(errors);
      if (errors.length) { setErrorBanner("Revise os campos da conexão antes de continuar."); return; }
      setIsValidating(true);
      try {
        const errors = await invoke<ValidationError[]>("validate_config", { config });
        const connectionErrors = errors?.filter((error) => ["bucket_name", "aws_region", "endpoint_url"].includes(error.field)) || [];
        if (connectionErrors.length) { setValidationErrors(connectionErrors); setErrorBanner(connectionErrors.map((error) => error.reason).join(" ")); return; }
      } catch { setErrorBanner("Não foi possível validar a conexão. Tente continuar novamente."); return; }
      finally { setIsValidating(false); }
    } else {
      if (!config.local_path.trim()) { setValidationErrors([{ field: "local_path", reason: "Escolha a pasta local." }]); return; }
      setIsValidating(true);
      try {
        const errors = await invoke<ValidationError[]>("validate_config", { config });
        if (errors?.length) { setValidationErrors(errors); setErrorBanner(errors.map((error) => error.reason).join(" ")); return; }
      } catch { setErrorBanner("Não foi possível validar a configuração. Tente continuar novamente."); return; }
      finally { setIsValidating(false); }
    }
    setValidationErrors([]);
    setSetupStep((step) => Math.min(2, step + 1));
  };

  return (
    <div className="pane-container">
      {/* Top Command Bar */}
      <header className="command-bar settings-command-bar">
        <div className="command-bar-left">
          <h2 className="pane-title">{firstSetup ? "Configuração inicial" : "Preferências do Sistema"}</h2>
        </div>
        <div className="command-bar-right">
          <button
            type="button"
            className="btn-compact"
            onClick={handleImportProfile}
            disabled={isDirty || busy}
            title={isDirty ? "Salve ou descarte as alterações antes de importar outro perfil." : "Carregar perfil JSON para revisão"}
          >
            Importar JSON
          </button>
          <button
            type="button"
            className="btn-compact"
            onClick={handleExportProfile}
            disabled={isDirty || busy}
            title={isDirty ? "Salve ou descarte as alterações antes de exportar." : "Exportar configuração salva (sem credenciais)"}
          >
            Exportar JSON
          </button>
          <div className="toolbar-divider" />
          <button
            type="button"
            className="btn-compact"
            onClick={handleSaveAll}
            disabled={busy || (firstSetup && setupStep !== 2)}
          >
            {isSaving ? "Salvando…" : "Salvar alterações"}
          </button>
          <button
            type="button"
            className="btn-primary-compact"
            disabled={!canStartSync || busy || (firstSetup && setupStep !== 2)}
            onClick={handleStartSync}
          >
            {syncAction === "starting" ? "Conectando…" : isDirty || firstSetup ? "Salvar e iniciar" : "Iniciar sincronização"}
          </button>
        </div>
      </header>

      {errorBanner && (
        <div role="alert" className="banner banner-error">
          {errorBanner}
        </div>
      )}
      {statusMessage && (
        <div role="status" className="banner banner-ok">
          {statusMessage}
        </div>
      )}

      <div className="settings-save-state" role="status">
        {isDirty ? "Alterações não salvas" : firstSetup ? "Complete as etapas para configurar a sincronização" : "Todas as alterações estão salvas"}
        {isDirty && <button type="button" className="btn-compact" disabled={busy} onClick={() => {
          setConfig(savedConfig);
          setCreds({ access_key_id: "", secret_access_key: "", session_token: "", region: savedConfig.aws_region, use_iam_role: savedConfig.use_iam_role });
          setValidationErrors([]); setErrorBanner(null); setStatusMessage(null);
        }}>Descartar alterações</button>}
      </div>
      <div className="settings-scroll">
        {firstSetup && <ol className="setup-steps" aria-label="Etapas da configuração inicial">
          {["Conexão", "Pasta e direção", "Revisão e início"].map((label, index) => <li key={label} aria-current={setupStep === index ? "step" : undefined}>{index + 1}. {label}</li>)}
        </ol>}
        <fieldset className="settings-fields" disabled={busy}>
        <section className="settings-group" hidden={firstSetup && setupStep !== 0}>
          <div className="settings-group-header">CONEXÃO COM O S3</div>
          <div className="prop-row">
            <div className="prop-label-col">
              <label htmlFor="bucket_name">Nome do Bucket S3</label>
              <span className="prop-sub">Identificador do bucket remoto</span>
            </div>
            <div className="prop-control-col">
              <div className="input-addon-row">
                <span className="addon-prefix mono">s3://</span>
                <input
                  id="bucket_name"
                  className="mono"
                  type="text"
                  placeholder="meu-bucket-s3"
                  value={config.bucket_name}
                  onChange={(e) =>
                    setConfig({ ...config, bucket_name: e.target.value })
                  }
                />
              </div>
              {getFieldError("bucket_name") && (
                <span className="field-error" data-testid="error-bucket_name">
                  {getFieldError("bucket_name")}
                </span>
              )}
            </div>
          </div>
          <div className="prop-row">
            <div className="prop-label-col">
              <label htmlFor="aws_region">Região AWS</label>
              <span className="prop-sub">Endpoint regional do S3</span>
            </div>
            <div className="prop-control-col">
              <div className="control-inline">
                <input
                  id="aws_region"
                  className="mono input-short"
                  type="text"
                  placeholder="us-east-1"
                  value={config.aws_region}
                  onChange={(e) => {
                    const region = e.target.value;
                    setConfig({ ...config, aws_region: region });
                    setCreds({ ...creds, region });
                  }}
                />
                <div className="preset-tags">
                  {["sa-east-1", "us-east-1", "us-west-2", "eu-west-1"].map(
                    (r) => (
                      <button
                        key={r}
                        type="button"
                        className={`tag-btn mono ${
                          config.aws_region === r ? "tag-btn-active" : ""
                        }`}
                        onClick={() => {
                          setConfig({ ...config, aws_region: r });
                          setCreds({ ...creds, region: r });
                        }}
                      >
                        {r}
                      </button>
                    )
                  )}
                </div>
              </div>
              {(getFieldError("aws_region") || getFieldError("region")) && (
                <span className="field-error" data-testid="error-region">
                  {getFieldError("aws_region") || getFieldError("region")}
                </span>
              )}
            </div>
          </div>
          <details className="settings-advanced" open={Boolean(getFieldError("endpoint_url")) || undefined}>
            <summary>Conexão avançada (endpoint personalizado)</summary>
          <div className="prop-row">
            <div className="prop-label-col">
              <label htmlFor="endpoint_url">Endpoint S3 Customizado</label>
              <span className="prop-sub">
                Opcional (Cloudflare R2, MinIO, Backblaze B2)
              </span>
            </div>
            <div className="prop-control-col">
              <input
                id="endpoint_url"
                className="mono"
                type="text"
                placeholder="https://<account>.r2.cloudflarestorage.com (vazio = AWS S3)"
                value={config.endpoint_url ?? ""}
                onChange={(e) =>
                  setConfig({
                    ...config,
                    endpoint_url: e.target.value ? e.target.value : null,
                  })
                }
              />
              <label htmlFor="force_path_style" className="check-inline">
                <input
                  id="force_path_style"
                  type="checkbox"
                  checked={Boolean(config.force_path_style)}
                  onChange={(e) =>
                    setConfig({
                      ...config,
                      force_path_style: e.target.checked,
                    })
                  }
                />
                <span>
                  Usar Path-Style Addressing (recomendado para MinIO / LocalStack)
                </span>
              </label>
              {getFieldError("endpoint_url") && (
                <span className="field-error" data-testid="error-endpoint_url">
                  {getFieldError("endpoint_url")}
                </span>
              )}
            </div>
          </div>
          </details>
        </section>

        {/* Bloco 2: Credenciais AWS */}
        <section className="settings-group" hidden={firstSetup && setupStep !== 0}>
          <div className="settings-group-header">
            <span>AUTENTICAÇÃO AWS (OS KEYRING)</span>
            <div className="header-inline-actions">
              <button
                type="button"
                className="btn-compact"
                disabled={busy || isDirty || firstSetup}
                title="Testa a configuração salva. Salve as alterações antes de testar."
                onClick={handleTestConnection}
              >
                {isTestingConn ? "Verificando..." : "Testar Conexão"}
              </button>
            </div>
          </div>

          <div className="prop-row">
            <div className="prop-label-col">
              <label htmlFor="use_iam_role">Modo de Autenticação</label>
              <span className="prop-sub">IAM Role do ambiente ou chaves</span>
            </div>
            <div className="prop-control-col">
              <label htmlFor="use_iam_role" className="check-inline">
                <input
                  id="use_iam_role"
                  type="checkbox"
                  checked={config.use_iam_role}
                  onChange={(e) => {
                    const useIam = e.target.checked;
                    setConfig({ ...config, use_iam_role: useIam });
                    setCreds({ ...creds, use_iam_role: useIam });
                  }}
                />
                <span>Usar AWS IAM Role / Perfil de Instância</span>
              </label>
            </div>
          </div>

          {!config.use_iam_role && (
            <>
              <div className="prop-row">
                <div className="prop-label-col">
                  <label htmlFor="access_key_id">Access Key ID</label>
                  <span className="prop-sub mono">
                    {creds.access_key_id.length}/20 chars
                  </span>
                </div>
                <div className="prop-control-col">
                  <input
                    id="access_key_id"
                    className="mono"
                    type="text"
                    placeholder="AKIAIOSFODNN7EXAMPLE"
                    value={creds.access_key_id}
                    onChange={(e) =>
                      setCreds({
                        ...creds,
                        access_key_id: e.target.value.toUpperCase(),
                      })
                    }
                  />
                  {getFieldError("access_key_id") && (
                    <span
                      className="field-error"
                      data-testid="error-access_key_id"
                    >
                      {getFieldError("access_key_id")}
                    </span>
                  )}
                </div>
              </div>

              <div className="prop-row">
                <div className="prop-label-col">
                  <label htmlFor="secret_access_key">Secret Access Key</label>
                  <span className="prop-sub mono">
                    {Array.from(creds.secret_access_key).length}/40 chars
                  </span>
                </div>
                <div className="prop-control-col">
                  <div className="control-inline">
                    <input
                      id="secret_access_key"
                      className="mono"
                      type={showSecretKey ? "text" : "password"}
                      placeholder="••••••••••••••••••••••••••••••••••••••••"
                      value={creds.secret_access_key}
                      onChange={(e) =>
                        setCreds({
                          ...creds,
                          secret_access_key: e.target.value,
                        })
                      }
                    />
                    <button
                      type="button"
                      className="btn-compact"
                      onClick={() => setShowSecretKey(!showSecretKey)}
                    >
                      {showSecretKey ? "Ocultar" : "Mostrar"}
                    </button>
                  </div>
                  {getFieldError("secret_access_key") && (
                    <span
                      className="field-error"
                      data-testid="error-secret_access_key"
                    >
                      {getFieldError("secret_access_key")}
                    </span>
                  )}
                </div>
              </div>

              <div className="prop-row">
                <div className="prop-label-col">
                  <label htmlFor="session_token">
                    Session Token STS (Opcional)
                  </label>
                  <span className="prop-sub">Credenciais temporárias AWS</span>
                </div>
                <div className="prop-control-col">
                  <input
                    id="session_token"
                    className="mono"
                    type="password"
                    placeholder="Token STS opcional"
                    value={creds.session_token ?? ""}
                    onChange={(e) =>
                      setCreds({ ...creds, session_token: e.target.value })
                    }
                  />
                </div>
              </div>
            </>
          )}
        </section>

        <section className="settings-group" hidden={firstSetup && setupStep !== 1}>
          <div className="settings-group-header">PASTA E DIREÇÃO</div>
          <div className="prop-row">
            <div className="prop-label-col">
              <label htmlFor="local_path">Pasta Local</label>
              <span className="prop-sub">Diretório espelhado no sistema</span>
            </div>
            <div className="prop-control-col">
              <div className="control-inline">
                <input
                  id="local_path"
                  className="mono"
                  type="text"
                  placeholder="C:\Users\Usuario\S3Drive"
                  value={config.local_path}
                  onChange={(e) =>
                    setConfig({ ...config, local_path: e.target.value })
                  }
                />
                <button
                  type="button"
                  className="btn-compact"
                  onClick={handleSelectPath}
                >
                  Selecionar Pasta
                </button>
              </div>
              {getFieldError("local_path") && (
                <span className="field-error" data-testid="error-local_path">
                  {getFieldError("local_path")}
                </span>
              )}
            </div>
          </div>
          <div className="prop-row">
            <div className="prop-label-col">
              <label htmlFor="s3_prefix">Prefixo S3 (Opcional)</label>
              <span className="prop-sub">Subdiretório remoto raiz</span>
            </div>
            <div className="prop-control-col">
              <input
                id="s3_prefix"
                className="mono"
                type="text"
                placeholder="ex: equipe/financeiro/"
                value={config.s3_prefix ?? ""}
                onChange={(e) =>
                  setConfig({
                    ...config,
                    s3_prefix: e.target.value ? e.target.value : null,
                  })
                }
              />
            </div>
          </div>
          <div className="prop-row">
            <div className="prop-label-col">
              <label htmlFor="sync_direction">Direção de Sincronização</label>
              <span className="prop-sub">
                Modo de espelhamento entre Pasta Local e Bucket S3
              </span>
            </div>
            <div className="prop-control-col">
              <div className="preset-tags">
                {[
                  {
                    id: "Bidirectional",
                    label: "Bidirecional (Local ↔ S3)",
                  },
                  {
                    id: "UploadOnly",
                    label: "Backup (Apenas Upload → S3)",
                  },
                  {
                    id: "DownloadOnly",
                    label: "Espelho (Apenas Download ← S3)",
                  },
                ].map((mode) => (
                  <button
                    key={mode.id}
                    type="button"
                    className={`tag-btn mono ${
                      (config.sync_direction ?? "Bidirectional") === mode.id
                        ? "tag-btn-active"
                        : ""
                    }`}
                    onClick={() =>
                      setConfig({
                        ...config,
                        sync_direction: mode.id,
                      })
                    }
                  >
                    {mode.label}
                  </button>
                ))}
              </div>
            </div>
          </div>
          <p className="settings-note">{config.sync_direction === "UploadOnly" ? "Envia alterações da pasta local ao S3." : config.sync_direction === "DownloadOnly" ? "Baixa alterações do S3 para a pasta local." : "Propaga alterações entre a pasta local e o S3 nos dois sentidos."} Revise a pasta e o destino antes de iniciar.</p>
        </section>

        {firstSetup && setupStep === 2 && (
          <section className="settings-group setup-review">
            <h3>Revise antes de iniciar</h3>
            <dl>
              <dt>Destino S3</dt><dd className="mono">s3://{config.bucket_name}/{config.s3_prefix || ""}</dd>
              <dt>Região</dt><dd>{config.aws_region}</dd>
              <dt>Pasta local</dt><dd className="mono">{config.local_path}</dd>
              <dt>Direção</dt><dd>{config.sync_direction === "UploadOnly" ? "Somente envio ao S3" : config.sync_direction === "DownloadOnly" ? "Somente download do S3" : "Bidirecional (Local ↔ S3)"}</dd>
              <dt>Autenticação</dt><dd>{config.use_iam_role ? "IAM Role do ambiente" : "Chaves no gerenciador seguro do sistema"}</dd>
            </dl>
            <p>Ao salvar e iniciar, a conexão será verificada e a sincronização poderá alterar arquivos nos destinos indicados.</p>
            <button type="button" className="btn-compact" disabled={busy || isDirty} onClick={handleTestConnection}>
              {isTestingConn ? "Verificando…" : "Testar conexão salva"}
            </button>
            {isDirty && <p>Salve as alterações para testar a conexão antes de iniciar.</p>}
          </section>
        )}
        <details className="settings-advanced" hidden={firstSetup && setupStep !== 2}>
          <summary>Ajustes avançados de sincronização</summary>
          <section className="settings-group">
            <div className="settings-group-header">TRANSFERÊNCIAS E ARMAZENAMENTO</div>
          <div className="prop-row">
            <div className="prop-label-col">
              <label htmlFor="sync_interval_secs">
                Intervalo de Sync (seg)
              </label>
              <span className="prop-sub">Frequência de polling (1–60s)</span>
            </div>
            <div className="prop-control-col">
              <input
                id="sync_interval_secs"
                className="mono input-xs"
                type="number"
                min={1}
                max={60}
                value={config.sync_interval_secs}
                onChange={(e) =>
                  setConfig({
                    ...config,
                    sync_interval_secs: Number(e.target.value) || 60,
                  })
                }
              />
            </div>
          </div>
          <div className="prop-row">
            <div className="prop-label-col">
              <label htmlFor="max_concurrent_transfers">
                Transferências Simultâneas
              </label>
              <span className="prop-sub">Worker pool paralelo (1–16)</span>
            </div>
            <div className="prop-control-col">
              <div className="control-inline">
                <input
                  id="max_concurrent_transfers"
                  className="mono input-xs"
                  type="number"
                  min={1}
                  max={16}
                  value={config.max_concurrent_transfers ?? 4}
                  onChange={(e) =>
                    setConfig({
                      ...config,
                      max_concurrent_transfers: Math.max(
                        1,
                        Math.min(16, Number(e.target.value) || 4)
                      ),
                    })
                  }
                />
                <div className="preset-tags">
                  {[1, 4, 8, 12].map((w) => (
                    <button
                      key={w}
                      type="button"
                      className={`tag-btn mono ${
                        (config.max_concurrent_transfers ?? 4) === w
                          ? "tag-btn-active"
                          : ""
                      }`}
                      onClick={() =>
                        setConfig({
                          ...config,
                          max_concurrent_transfers: w,
                        })
                      }
                    >
                      {w}x
                    </button>
                  ))}
                </div>
              </div>
            </div>
          </div>
          <div className="prop-row">
            <div className="prop-label-col">
              <label htmlFor="storage_class">Storage Class & Segurança</label>
              <span className="prop-sub">
                Classe de armazenamento S3 e criptografia SSE-S3
              </span>
            </div>
            <div className="prop-control-col">
              <div className="preset-tags">
                {[
                  { id: "STANDARD", label: "STANDARD" },
                  { id: "INTELLIGENT_TIERING", label: "INTELLIGENT_TIERING" },
                  { id: "STANDARD_IA", label: "STANDARD_IA" },
                  { id: "GLACIER_IR", label: "GLACIER_IR" },
                ].map((sc) => (
                  <button
                    key={sc.id}
                    type="button"
                    className={`tag-btn mono ${
                      (config.storage_class ?? "STANDARD") === sc.id
                        ? "tag-btn-active"
                        : ""
                    }`}
                    onClick={() =>
                      setConfig({
                        ...config,
                        storage_class: sc.id,
                      })
                    }
                  >
                    {sc.label}
                  </button>
                ))}
              </div>
              <label htmlFor="server_side_encryption" className="check-inline">
                <input
                  id="server_side_encryption"
                  type="checkbox"
                  checked={Boolean(config.server_side_encryption)}
                  onChange={(e) =>
                    setConfig({
                      ...config,
                      server_side_encryption: e.target.checked,
                    })
                  }
                />
                <span>
                  Habilitar criptografia Server-Side AES-256 (SSE-S3) nos uploads
                </span>
              </label>
            </div>
          </div>
          <div className="prop-row">
            <div className="prop-label-col">
              <label htmlFor="bandwidth_limit_kbps">
                Limite de Banda (KB/s)
              </label>
              <span className="prop-sub">
                0 = ilimitado (Throttling de rede)
              </span>
            </div>
            <div className="prop-control-col">
              <div className="control-inline">
                <input
                  id="bandwidth_limit_kbps"
                  className="mono input-short"
                  type="number"
                  min={0}
                  step={512}
                  value={config.bandwidth_limit_kbps ?? 0}
                  onChange={(e) =>
                    setConfig({
                      ...config,
                      bandwidth_limit_kbps: Math.max(
                        0,
                        Number(e.target.value) || 0
                      ),
                    })
                  }
                />
                <div className="preset-tags">
                  {[
                    { val: 0, label: "Ilimitado" },
                    { val: 1024, label: "1 MB/s" },
                    { val: 5120, label: "5 MB/s" },
                    { val: 10240, label: "10 MB/s" },
                  ].map((bw) => (
                    <button
                      key={bw.val}
                      type="button"
                      className={`tag-btn mono ${
                        (config.bandwidth_limit_kbps ?? 0) === bw.val
                          ? "tag-btn-active"
                          : ""
                      }`}
                      onClick={() =>
                        setConfig({
                          ...config,
                          bandwidth_limit_kbps: bw.val,
                        })
                      }
                    >
                      {bw.label}
                    </button>
                  ))}
                </div>
              </div>
            </div>
          </div>
          </section>
        {/* Bloco 3: Filtros de Exclusão Glob */}
        <section className="settings-group">
          <div className="settings-group-header">
            FILTROS DE EXCLUSÃO (GLOB PATTERNS)
          </div>

          <div className="prop-row">
            <div className="prop-label-col">
              <label>Regras de Ignorados</label>
              <span className="prop-sub">Omitidos no upload e download</span>
            </div>
            <div className="prop-control-col">
              <div className="control-inline">
                <input
                  aria-label="Novo padrão de exclusão"
                  className="mono"
                  type="text"
                  placeholder="Ex: *.tmp, *.log, .DS_Store"
                  value={newPattern}
                  onChange={(e) => setNewPattern(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      handleAddPattern();
                    }
                  }}
                />
                <button
                  type="button"
                  className="btn-compact"
                  onClick={handleAddPattern}
                >
                  Adicionar Padrão
                </button>
              </div>

              <div className="preset-tags">
                {QUICK_EXCLUSION_PRESETS.map((preset) => (
                  <button
                    key={preset}
                    type="button"
                    className="tag-btn mono"
                    onClick={() => handleAddPatternValue(preset)}
                  >
                    +{preset}
                  </button>
                ))}
              </div>

              {getFieldError("exclusion_patterns") && (
                <span
                  className="field-error"
                  data-testid="error-exclusion_patterns"
                >
                  {getFieldError("exclusion_patterns")}
                </span>
              )}

              {config.exclusion_patterns.length > 0 && (
                <ul className="pattern-list">
                  {config.exclusion_patterns.map((pat) => (
                    <li key={pat} className="pattern-chip mono">
                      <code>{pat}</code>
                      <button
                        type="button"
                        className="chip-del"
                        aria-label={`Remover ${pat}`}
                        onClick={() => handleRemovePattern(pat)}
                      >
                        ×
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </div>
        </section>

        {/* Bloco 4: Notificações */}
        <section className="settings-group">
          <div className="settings-group-header">EVENTOS & NOTIFICAÇÕES</div>

          <div className="prop-row">
            <div className="prop-label-col">
              <span>Alertas do Sistema</span>
              <span className="prop-sub">Comportamento de bandeja e UI</span>
            </div>
            <div className="prop-control-col check-stack">
              <label htmlFor="notif_sync_complete" className="check-inline">
                <input
                  id="notif_sync_complete"
                  type="checkbox"
                  checked={config.notifications.sync_complete_enabled}
                  onChange={(e) =>
                    handleNotificationToggle(
                      "sync_complete_enabled",
                      e.target.checked
                    )
                  }
                />
                <span>
                  Notificar ao concluir a primeira sincronização completa
                </span>
              </label>

              <label htmlFor="notif_conflict" className="check-inline">
                <input
                  id="notif_conflict"
                  type="checkbox"
                  checked={config.notifications.conflict_enabled}
                  onChange={(e) =>
                    handleNotificationToggle(
                      "conflict_enabled",
                      e.target.checked
                    )
                  }
                />
                <span>Notificar ao detectar conflitos de arquivos</span>
              </label>

              <label
                htmlFor="notif_error"
                className="check-inline check-disabled"
              >
                <input
                  id="notif_error"
                  type="checkbox"
                  checked={true}
                  disabled={true}
                  readOnly
                />
                <span>Notificar erros de sincronização (Sempre ativo)</span>
              </label>

              <label htmlFor="launch_on_startup" className="check-inline">
                <input
                  id="launch_on_startup"
                  type="checkbox"
                  checked={Boolean(config.launch_on_startup)}
                  onChange={(e) =>
                    setConfig({
                      ...config,
                      launch_on_startup: e.target.checked,
                    })
                  }
                />
                <span>
                  Iniciar Amazon S3 Drive automaticamente ao ligar o sistema (Autostart)
                </span>
              </label>
            </div>
          </div>
        </section>
        </details>
        </fieldset>
        {firstSetup && <div className="setup-actions">
          <button type="button" className="btn-compact" disabled={setupStep === 0 || busy} onClick={() => { setSetupStep((step) => step - 1); setValidationErrors([]); setErrorBanner(null); }}>Voltar</button>
          {setupStep < 2 && <button type="button" className="btn-primary-compact" disabled={busy} onClick={handleNextStep}>Continuar</button>}
        </div>}
      </div>
    </div>
  );
};
