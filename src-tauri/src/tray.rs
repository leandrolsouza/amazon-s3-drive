use crate::language::tr;
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
pub(crate) const MAIN_TRAY_ID: &str = "main-tray";
pub const MENU_SETTINGS_ID: &str = "settings";
pub const MENU_QUIT_ID: &str = "quit";

pub fn tray_tooltip_for_state(state: &GlobalSyncState) -> &'static str {
    match state {
        GlobalSyncState::UpToDate => tr("Amazon S3 Drive — Up to date", "Amazon S3 Drive — Em dia"),
        GlobalSyncState::Syncing => tr("Amazon S3 Drive — Syncing...", "Amazon S3 Drive — Sincronizando..."),
        GlobalSyncState::Error => tr("Amazon S3 Drive — Sync error", "Amazon S3 Drive — Erro de sincronização"),
        GlobalSyncState::Offline => "Amazon S3 Drive — Offline",
    }
}

pub(crate) fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn tray_icon(logo: &Image<'_>, color: [u8; 4]) -> Image<'static> {
    let mut rgba = logo.rgba().to_vec();
    // Draw a dark outline so the status dot stays visible over the logo.
    let center = 24;
    for y in 17..=31 {
        for x in 17..=31 {
            let dx = x - center;
            let dy = y - center;
            let distance_squared = dx * dx + dy * dy;
            if distance_squared <= 49 {
                let pixel = if distance_squared <= 25 {
                    color
                } else {
                    [17, 24, 39, 255]
                };
                let offset = (y as usize * logo.width() as usize + x as usize) * 4;
                rgba[offset..offset + 4].copy_from_slice(&pixel);
            }
        }
    }
    Image::new_owned(rgba, logo.width(), logo.height())
}

pub fn setup_system_tray(
    app: &AppHandle,
    status_monitor: Arc<StatusMonitor>,
) -> Result<(), tauri::Error> {
    let open_item = MenuItem::with_id(app, MENU_OPEN_DRIVE_ID, tr("Open S3 Drive", "Abrir S3 Drive"), true, None::<&str>)?;
    let settings_item = MenuItem::with_id(app, MENU_SETTINGS_ID, tr("Settings", "Configurações"), true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, MENU_QUIT_ID, tr("Quit", "Sair"), true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&open_item, &settings_item, &sep, &quit_item])?;

    let logo = Image::from_bytes(include_bytes!("../icons/32x32.png"))?;
    let up_to_date_icon = tray_icon(&logo, [34, 197, 94, 255]);
    let error_icon = tray_icon(&logo, [239, 68, 68, 255]);
    let offline_icon = tray_icon(&logo, [156, 163, 175, 255]);
    let syncing_icons = [
        tray_icon(&logo, [96, 165, 250, 255]),
        tray_icon(&logo, [59, 130, 246, 255]),
    ];

    let tray = TrayIconBuilder::with_id(MAIN_TRAY_ID)
        .icon(up_to_date_icon.clone())
        .tooltip(tray_tooltip_for_state(&GlobalSyncState::UpToDate))
        .menu(&menu)
        .on_menu_event(|app_handle, event| match event.id().as_ref() {
            MENU_OPEN_DRIVE_ID => {
                show_main_window(app_handle);
                let _ = app_handle.emit("tray:navigate", "status");
            }
            MENU_SETTINGS_ID => {
                show_main_window(app_handle);
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
                    let _ = tray.set_icon(Some(up_to_date_icon.clone()));
                }
                GlobalSyncState::Error => {
                    animating.store(false, Ordering::SeqCst);
                    let _ = tray.set_icon(Some(error_icon.clone()));
                }
                GlobalSyncState::Offline => {
                    animating.store(false, Ordering::SeqCst);
                    let _ = tray.set_icon(Some(offline_icon.clone()));
                }
                GlobalSyncState::Syncing => {
                    if !animating.swap(true, Ordering::SeqCst) {
                        let tray_anim = tray.clone();
                        let anim_flag = Arc::clone(&animating);
                        let icons = syncing_icons.clone();
                        tokio::spawn(async move {
                            let mut frame = 0;
                            while anim_flag.load(Ordering::SeqCst) {
                                let _ = tray_anim.set_icon(Some(icons[frame].clone()));
                                frame ^= 1;
                                tokio::time::sleep(Duration::from_millis(500)).await;
                            }
                        });
                    }
                }
            }
        }
    };

    let update_tray = refresh_tray.clone();
    app.listen("app:language-changed", move |_| {
        let _ = open_item.set_text(tr("Open S3 Drive", "Abrir S3 Drive"));
        let _ = settings_item.set_text(tr("Settings", "Configurações"));
        let _ = quit_item.set_text(tr("Quit", "Sair"));
        update_tray();
    });

    let r1 = refresh_tray.clone();
    app.listen("sync:file-status-changed", move |_| r1());

    let r2 = refresh_tray.clone();
    app.listen("network:offline", move |_| r2());

    let r3 = refresh_tray;
    app.listen("network:online", move |_| r3());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_icon_preserves_logo_and_displays_status_color() {
        let logo = Image::from_bytes(include_bytes!("../icons/32x32.png")).unwrap();
        let color = [34, 197, 94, 255];
        let icon = tray_icon(&logo, color);
        let row_bytes = logo.width() as usize * 4;
        let badge_center = 24 * row_bytes + 24 * 4;

        assert_eq!((icon.width(), icon.height()), (32, 32));
        assert_eq!(&icon.rgba()[..17 * row_bytes], &logo.rgba()[..17 * row_bytes]);
        assert_eq!(icon.rgba()[3], 0);
        assert_eq!(&icon.rgba()[badge_center..badge_center + 4], &color);
    }
}
