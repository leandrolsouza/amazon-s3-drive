use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

static PORTUGUESE: AtomicBool = AtomicBool::new(false);

fn preference_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("amazon-s3-drive")
        .join("language")
}

pub fn load() {
    let preference = std::fs::read_to_string(preference_path());
    PORTUGUESE.store(
        preference.as_deref().ok() == Some("pt-BR"),
        Ordering::Relaxed,
    );
}

pub fn tr(english: &'static str, portuguese: &'static str) -> &'static str {
    if PORTUGUESE.load(Ordering::Relaxed) {
        portuguese
    } else {
        english
    }
}

#[tauri::command]
pub fn get_language() -> &'static str {
    tr("en-US", "pt-BR")
}

#[tauri::command]
pub fn set_language(app: tauri::AppHandle, language: &str) -> Result<(), String> {
    if !matches!(language, "en-US" | "pt-BR") {
        return Err(tr("Unsupported language.", "Idioma incompatível.").into());
    }
    std::fs::write(preference_path(), language).map_err(|_| {
        tr(
            "Could not save the language preference.",
            "Não foi possível salvar a preferência de idioma.",
        )
    })?;
    PORTUGUESE.store(language == "pt-BR", Ordering::Relaxed);
    use tauri::Emitter;
    let _ = app.emit("app:language-changed", ());
    Ok(())
}

macro_rules! localized_format {
    ($english:literal, $portuguese:literal $(, $argument:expr)* $(,)?) => {
        if $crate::language::get_language() == "pt-BR" {
            format!($portuguese $(, $argument)*)
        } else {
            format!($english $(, $argument)*)
        }
    };
}
pub(crate) use localized_format;
