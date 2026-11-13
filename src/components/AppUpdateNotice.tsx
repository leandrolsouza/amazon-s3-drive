import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { t } from "../i18n";

export function AppUpdateNotice() {
  const [version, setVersion] = useState<string | null>(null);
  const [installing, setInstalling] = useState(false);
  const [error, setError] = useState(false);

  useEffect(() => {
    let disposed = false;
    invoke<string | null>("check_app_update")
      .then((available) => { if (!disposed && available) setVersion(available); })
      .catch(() => {});
    return () => { disposed = true; };
  }, []);

  if (!version) return null;

  async function install() {
    setInstalling(true);
    setError(false);
    try {
      await invoke("install_app_update", { version });
    } catch {
      setInstalling(false);
      setError(true);
    }
  }

  return (
    <div className={error ? "banner banner-error" : "banner"}>
      <span role={error ? "alert" : "status"}>
        {error ? t("Could not update the app. Try again or reopen the app.")
          : installing ? t("Preparing the update. The app will restart.")
          : t("Version {0} is available.", version)}
      </span>
      <button type="button" disabled={installing} onClick={install}>{t("Update and restart")}</button>
      <button type="button" disabled={installing} onClick={() => setVersion(null)}>{t("Not now")}</button>
    </div>
  );
}
