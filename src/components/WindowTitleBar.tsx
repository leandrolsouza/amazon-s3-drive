import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Icon } from "./Icon";
import { t } from "../i18n";

export function WindowTitleBar() {
  const [appWindow] = useState(getCurrentWindow);
  const [maximized, setMaximized] = useState(false);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let revision = 0;
    const refresh = () => {
      const currentRevision = ++revision;
      void appWindow.isMaximized().then((value) => {
        if (!disposed && currentRevision === revision) setMaximized(value);
      }).catch(() => {});
    };

    refresh();
    void appWindow.onResized(refresh).then((unsubscribe) => {
      if (disposed) unsubscribe();
      else {
        unlisten = unsubscribe;
        refresh();
      }
    }).catch(() => {});

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [appWindow]);

  const run = async (action: () => Promise<void>) => {
    setFailed(false);
    try {
      await action();
    } catch {
      setFailed(true);
    }
  };

  const maximizeLabel = t(maximized ? "Restore window" : "Maximize window");
  return (
    <>
      <header className="window-titlebar">
        <div className="window-titlebar-drag" data-tauri-drag-region>
          <span className="window-titlebar-mark" aria-hidden="true"><Icon name="bucket" /></span>
          <h1 data-tauri-drag-region>Amazon S3 Drive</h1>
        </div>
        <div className="window-controls" role="group" aria-label={t("Window controls")}>
          <button type="button" aria-label={t("Minimize window")} title={t("Minimize window")}
            onClick={() => void run(() => appWindow.minimize())}>
            <Icon name="minimize" />
          </button>
          <button type="button" aria-label={maximizeLabel} title={maximizeLabel}
            onClick={() => void run(() => appWindow.toggleMaximize())}>
            <Icon name={maximized ? "restore" : "maximize"} />
          </button>
          <button type="button" className="window-close" aria-label={t("Close window")} title={t("Close window")}
            onClick={() => void run(() => appWindow.close())}>
            <Icon name="close" />
          </button>
        </div>
      </header>
      {failed && <p className="window-control-error" role="alert">{t("Could not change the window state. Try again.")}</p>}
    </>
  );
}
