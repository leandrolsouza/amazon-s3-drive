use crate::models::GlobalSyncState;
use crate::status_monitor::StatusMonitor;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Listener, Manager,
};

pub const MENU_OPEN_DRIVE_ID: &str = "open_drive";
pub const MENU_SETTINGS_ID: &str = "settings";
pub const MENU_QUIT_ID: &str = "quit";

pub fn tray_tooltip_for_state(state: &GlobalSyncState) -> &'static str {
    match state {
        GlobalSyncState::UpToDate => "Amazon S3 Drive — Em dia",
        GlobalSyncState::Syncing => "Amazon S3 Drive — Sincronizando...",
        GlobalSyncState::Error => "Amazon S3 Drive — Erro de sincronização",
        GlobalSyncState::Offline => "Amazon S3 Drive — Offline",
    }
}

/// Gera um ícone RGBA 16x16 sólido para cada estado do tray (verde, azul frame 1/2, vermelho, cinza)
pub fn make_solid_rgba_icon(r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(16 * 16 * 4);
    for _ in 0..(16 * 16) {
        rgba.push(r);
        rgba.push(g);
        rgba.push(b);
        rgba.push(255);
    }
    rgba
}

pub fn setup_system_tray(
    app: &AppHandle,
    status_monitor: Arc<StatusMonitor>,
) -> Result<(), tauri::Error> {
    let open_item = MenuItem::with_id(app, MENU_OPEN_DRIVE_ID, "Abrir S3 Drive", true, None::<&str>)?;
    let settings_item = MenuItem::with_id(app, MENU_SETTINGS_ID, "Configurações", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, MENU_QUIT_ID, "Sair", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&open_item, &settings_item, &sep, &quit_item])?;

    let green_rgba = make_solid_rgba_icon(34, 197, 94);
    let initial_icon = Image::new_owned(green_rgba, 16, 16);

    let tray = TrayIconBuilder::with_id("main-tray")
        .icon(initial_icon)
        .tooltip(tray_tooltip_for_state(&GlobalSyncState::UpToDate))
        .menu(&menu)
        .on_menu_event(|app_handle, event| match event.id().as_ref() {
            MENU_OPEN_DRIVE_ID => {
                if let Some(w) = app_handle.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
                let _ = app_handle.emit("tray:navigate", "status");
            }
            MENU_SETTINGS_ID => {
                if let Some(w) = app_handle.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
                let _ = app_handle.emit("tray:navigate", "config");
            }
            MENU_QUIT_ID => {
                app_handle.exit(0);
            }
            _ => {}
        })
        .build(app)?;

    let animating = Arc::new(AtomicBool::new(false));

    let refresh_tray = {
        let tray = tray.clone();
        let monitor = Arc::clone(&status_monitor);
        let animating = Arc::clone(&animating);
        move || {
            let state = monitor.global_state();
            let _ = tray.set_tooltip(Some(tray_tooltip_for_state(&state)));

            match state {
                GlobalSyncState::UpToDate => {
                    animating.store(false, Ordering::SeqCst);
                    let icon = Image::new_owned(make_solid_rgba_icon(34, 197, 94), 16, 16);
                    let _ = tray.set_icon(Some(icon));
                }
                GlobalSyncState::Error => {
                    animating.store(false, Ordering::SeqCst);
                    let icon = Image::new_owned(make_solid_rgba_icon(239, 68, 68), 16, 16);
                    let _ = tray.set_icon(Some(icon));
                }
                GlobalSyncState::Offline => {
                    animating.store(false, Ordering::SeqCst);
                    let icon = Image::new_owned(make_solid_rgba_icon(156, 163, 175), 16, 16);
                    let _ = tray.set_icon(Some(icon));
                }
                GlobalSyncState::Syncing => {
                    if !animating.swap(true, Ordering::SeqCst) {
                        let tray_anim = tray.clone();
                        let anim_flag = Arc::clone(&animating);
                        tokio::spawn(async move {
                            let mut frame = false;
                            while anim_flag.load(Ordering::SeqCst) {
                                let rgba = if frame {
                                    make_solid_rgba_icon(59, 130, 246)
                                } else {
                                    make_solid_rgba_icon(96, 165, 250)
                                };
                                let _ = tray_anim.set_icon(Some(Image::new_owned(rgba, 16, 16)));
                                frame = !frame;
                                tokio::time::sleep(Duration::from_millis(500)).await;
                            }
                        });
                    }
                }
            }
        }
    };

    let r1 = refresh_tray.clone();
    app.listen("sync:file-status-changed", move |_| r1());

    let r2 = refresh_tray.clone();
    app.listen("network:offline", move |_| r2());

    let r3 = refresh_tray;
    app.listen("network:online", move |_| r3());

    Ok(())
}
