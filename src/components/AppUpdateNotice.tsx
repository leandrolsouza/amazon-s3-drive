import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

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
        {error ? "Não foi possível atualizar o aplicativo. Tente novamente ou reabra o app."
          : installing ? "Preparando a atualização. O aplicativo será reiniciado."
          : `Versão ${version} disponível.`}
      </span>
      <button type="button" disabled={installing} onClick={install}>Atualizar e reiniciar</button>
      <button type="button" disabled={installing} onClick={() => setVersion(null)}>Agora não</button>
    </div>
  );
}
