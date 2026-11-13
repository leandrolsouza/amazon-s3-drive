use crate::language::tr;
use super::{aws_error, s3_client, sdk_config};
use crate::AppState;
use aws_sdk_s3::{
    error::ProvideErrorMetadata,
    primitives::ByteStream,
    types::{AnnotationDirective, MetadataDirective, TaggingDirective},
    Client,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use tauri::State;

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SiteSource {
    Local { folder: String },
    S3 { prefix: String },
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SiteFile {
    key: String,
    size: u64,
    revision: String,
}

enum SourceFile {
    Local { root: PathBuf, path: PathBuf },
    S3 { bucket: String, key: String },
}

struct Candidate {
    file: SiteFile,
    source: SourceFile,
}

#[tauri::command]
pub async fn preview_site_source(
    source: SiteSource,
    state: State<'_, AppState>,
) -> Result<Vec<SiteFile>, String> {
    let config = state
        .config_parser
        .load()
        .map_err(|_| tr("Save your AWS settings first.", "Salve a configuração AWS primeiro."))?;
    let s3 = if matches!(&source, SiteSource::S3 { .. }) {
        Some(s3_client(&sdk_config(&state, &config).await?))
    } else {
        None
    };
    Ok(candidates(source, &config, s3.as_ref())
        .await?
        .into_iter()
        .map(|candidate| candidate.file)
        .collect())
}

#[derive(Serialize)]
pub struct SiteFolder {
    pub prefix: String,
    pub folders: Vec<String>,
    pub has_index: bool,
}

#[tauri::command]
pub async fn browse_site_source(
    prefix: String,
    state: State<'_, AppState>,
) -> Result<SiteFolder, String> {
    let config = state
        .config_parser
        .load()
        .map_err(|_| tr("Save your AWS settings first.", "Salve a configuração AWS primeiro."))?;
    let s3 = s3_client(&sdk_config(&state, &config).await?);
    let (bucket, base) = remote_location(&config, &prefix)?;
    let mut pages = s3
        .list_objects_v2()
        .bucket(bucket)
        .prefix(&base)
        .delimiter("/")
        .into_paginator()
        .send();
    let mut folders = Vec::new();
    let index = format!("{base}index.html");
    let mut has_index = false;
    while let Some(page) = pages.next().await {
        let page =
            page.map_err(|error| aws_error(tr("browse the synced bucket", "navegar pelo bucket sincronizado"), error.code()))?;
        has_index |= page
            .contents()
            .iter()
            .any(|item| item.key() == Some(&index));
        for folder in page
            .common_prefixes()
            .iter()
            .filter_map(|entry| entry.prefix())
        {
            if let Some(name) = folder
                .strip_prefix(&base)
                .and_then(|name| name.strip_suffix('/'))
            {
                if valid_key(name) && !name.contains('/') {
                    folders.push(name.to_owned());
                }
            }
        }
    }
    folders.sort();
    folders.dedup();
    Ok(SiteFolder {
        prefix,
        folders,
        has_index,
    })
}

fn remote_location<'a>(
    config: &'a crate::models::AppConfig,
    prefix: &str,
) -> Result<(&'a str, String), String> {
    let bucket = config.bucket_name.trim();
    if bucket.is_empty() {
        return Err(tr("Set up the sync bucket first.", "Configure o bucket de sincronização primeiro.").into());
    }
    if !prefix.is_empty() && !valid_key(prefix) {
        return Err(tr("Invalid bucket folder.", "Pasta do bucket inválida.").into());
    }
    let base = config.s3_prefix.as_deref().unwrap_or("").trim_matches('/');
    let mut parts = [base, prefix]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if !parts.is_empty() {
        parts.push('/');
    }
    Ok((bucket, parts))
}

fn valid_key(key: &str) -> bool {
    key.len() <= 982
        && !key.contains('\\')
        && !key.chars().any(char::is_control)
        && key
            .split('/')
            .all(|segment| !segment.is_empty() && !segment.starts_with('.'))
}

async fn candidates(
    source: SiteSource,
    config: &crate::models::AppConfig,
    s3: Option<&Client>,
) -> Result<Vec<Candidate>, String> {
    let mut files = match source {
        SiteSource::Local { folder } => {
            let (root, entries) = collect_files(Path::new(&folder)).await?;
            entries
                .into_iter()
                .map(|(path, file)| Candidate {
                    file,
                    source: SourceFile::Local {
                        root: root.clone(),
                        path,
                    },
                })
                .collect()
        }
        SiteSource::S3 { prefix } => {
            let (bucket, base) = remote_location(config, &prefix)?;
            let s3 = s3.ok_or(tr("The S3 connection is unavailable.", "Conexão com o S3 indisponível."))?;
            let mut pages = s3
                .list_objects_v2()
                .bucket(bucket)
                .prefix(&base)
                .into_paginator()
                .send();
            let mut files = Vec::new();
            while let Some(page) = pages.next().await {
                let page =
                    page.map_err(|error| aws_error(tr("list the bucket files", "listar os arquivos do bucket"), error.code()))?;
                for entry in page.contents() {
                    let Some(key) = entry.key() else { continue };
                    let Some(relative) = key.strip_prefix(&base).filter(|key| valid_key(key))
                    else {
                        continue;
                    };
                    let size = entry
                        .size()
                        .and_then(|size| u64::try_from(size).ok())
                        .ok_or(tr("S3 object size unavailable.", "Tamanho de objeto S3 indisponível."))?;
                    let revision = entry.e_tag().ok_or(tr("S3 object version unavailable.", "Versão de objeto S3 indisponível."))?;
                    files.push(Candidate {
                        file: SiteFile {
                            key: relative.to_owned(),
                            size,
                            revision: revision.to_owned(),
                        },
                        source: SourceFile::S3 {
                            bucket: bucket.to_owned(),
                            key: key.to_owned(),
                        },
                    });
                }
            }
            files
        }
    };
    if !files
        .iter()
        .any(|candidate| candidate.file.key == "index.html")
    {
        return Err(tr("The folder must contain index.html at its root.", "A pasta precisa conter um arquivo index.html na raiz.").into());
    }
    files.sort_by(|a, b| a.file.key.cmp(&b.file.key));
    Ok(files)
}

pub(super) struct Selection(Vec<Candidate>);

impl Selection {
    pub async fn validate(
        source: SiteSource,
        selected: Vec<SiteFile>,
        config: &crate::models::AppConfig,
        s3: &Client,
    ) -> Result<Self, String> {
        if !selected.iter().any(|file| file.key == "index.html") {
            return Err(tr("Include index.html in the selection.", "Inclua o index.html na seleção.").into());
        }
        let mut current: HashMap<_, _> = candidates(source, config, Some(s3))
            .await?
            .into_iter()
            .map(|candidate| (candidate.file.key.clone(), candidate))
            .collect();
        let files = selected
            .into_iter()
            .map(|file| {
                let candidate = current
                    .remove(&file.key)
                    .filter(|entry| entry.file == file)
                    .ok_or(tr("The selection changed since the review. Review the files again.", "A seleção mudou desde a revisão. Revise os arquivos novamente."))?;
                if file.size > 5_000_000_000 {
                    return Err(tr("The selection contains a file larger than 5 GB.", "A seleção contém um arquivo maior que 5 GB."));
                }
                Ok(candidate)
            })
            .collect::<Result<_, _>>()?;
        Ok(Self(files))
    }

    pub async fn upload(self, s3: &Client, bucket: &str, release: &str) -> Result<(), String> {
        for Candidate { file, source } in self.0 {
            let target = format!("releases/{release}/{}", file.key);
            let mime = mime_guess::from_path(&file.key).first_or_octet_stream();
            match source {
                SourceFile::Local { root, path } => {
                    let resolved = tokio::fs::canonicalize(&path)
                        .await
                        .map_err(|_| tr("A file changed during publishing.", "Um arquivo mudou durante a publicação."))?;
                    if resolved != path || !resolved.starts_with(&root) {
                        return Err(
                            tr("A file changed during publishing. Review the selection.", "Um arquivo mudou durante a publicação. Revise a seleção.").into()
                        );
                    }
                    let source = tokio::fs::File::open(&resolved)
                        .await
                        .map_err(|_| tr("Could not open a site file.", "Não foi possível abrir um arquivo do site."))?;
                    let metadata = source
                        .metadata()
                        .await
                        .map_err(|_| tr("Could not verify a site file.", "Não foi possível verificar um arquivo do site."))?;
                    if !metadata.is_file()
                        || metadata.len() != file.size
                        || local_revision(&metadata)? != file.revision
                    {
                        return Err(
                            tr("A file changed during publishing. Review the selection.", "Um arquivo mudou durante a publicação. Revise a seleção.").into()
                        );
                    }
                    let body = ByteStream::read_from()
                        .file(source)
                        .build()
                        .await
                        .map_err(|_| tr("Could not prepare a site file.", "Não foi possível preparar um arquivo do site."))?;
                    s3.put_object()
                        .bucket(bucket)
                        .key(target)
                        .content_type(mime.as_ref())
                        .cache_control("no-cache")
                        .body(body)
                        .send()
                        .await
                        .map_err(|error| aws_error(tr("upload the site files", "enviar os arquivos do site"), error.code()))?;
                }
                SourceFile::S3 {
                    bucket: origin,
                    key,
                } => {
                    let metadata = s3
                        .head_object()
                        .bucket(&origin)
                        .key(&key)
                        .if_match(&file.revision)
                        .send()
                        .await
                        .map_err(|error| copy_error(error.code()))?;
                    let content_type = mime_guess::from_path(&file.key)
                        .first()
                        .map(|mime| mime.to_string())
                        .or_else(|| metadata.content_type().map(str::to_owned))
                        .unwrap_or_else(|| mime.to_string());
                    s3.copy_object()
                        .bucket(bucket)
                        .key(target)
                        .copy_source(copy_source(&origin, &key))
                        .copy_source_if_match(file.revision)
                        .metadata_directive(MetadataDirective::Replace)
                        .tagging_directive(TaggingDirective::Replace)
                        .annotation_directive(AnnotationDirective::Exclude)
                        .content_type(content_type)
                        .set_content_encoding(metadata.content_encoding().map(str::to_owned))
                        .set_content_language(metadata.content_language().map(str::to_owned))
                        .cache_control("no-cache")
                        .send()
                        .await
                        .map_err(|error| copy_error(error.code()))?;
                }
            }
        }
        Ok(())
    }
}

fn copy_error(code: Option<&str>) -> String {
    if matches!(code, Some("PreconditionFailed" | "NoSuchKey" | "NotFound")) {
        return tr("A file changed in S3. Review the selection again.", "Um arquivo mudou no S3. Revise a seleção novamente.").into();
    }
    aws_error(tr("copy the site files", "copiar os arquivos do site"), code)
}

fn copy_source(bucket: &str, key: &str) -> String {
    use std::fmt::Write;
    let mut value = format!("{bucket}/");
    for byte in key.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~/".contains(&byte) {
            value.push(char::from(byte));
        } else {
            let _ = write!(value, "%{byte:02X}");
        }
    }
    value
}

fn local_revision(metadata: &std::fs::Metadata) -> Result<String, String> {
    let modified = metadata
        .modified()
        .map_err(|_| tr("File timestamp unavailable.", "Data do arquivo indisponível."))?;
    Ok(format!("{modified:?}"))
}

async fn collect_files(folder: &Path) -> Result<(PathBuf, Vec<(PathBuf, SiteFile)>), String> {
    if !folder.is_absolute() {
        return Err(tr("Enter the full path to the site folder.", "Informe o caminho completo da pasta do site.").into());
    }
    let root = tokio::fs::canonicalize(folder)
        .await
        .map_err(|_| tr("The site folder is inaccessible.", "A pasta do site está inacessível."))?;
    if root.parent().is_none() {
        return Err(tr("Select a site folder on the drive.", "Selecione uma pasta do site dentro da unidade.").into());
    }
    let mut pending = vec![(root.clone(), String::new())];
    let mut files = Vec::new();
    while let Some((directory, prefix)) = pending.pop() {
        let mut entries = tokio::fs::read_dir(&directory)
            .await
            .map_err(|_| tr("Could not read the site folder.", "Não foi possível ler a pasta do site."))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|_| tr("Could not list the site files.", "Não foi possível listar os arquivos do site."))?
        {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| tr("The folder contains a filename incompatible with URLs.", "A pasta contém um nome de arquivo incompatível com URLs."))?;
            let path = entry.path();
            let metadata = tokio::fs::symlink_metadata(&path)
                .await
                .map_err(|_| tr("Could not read an item in the site folder.", "Não foi possível ler um item da pasta do site."))?;
            if name.starts_with('.') || hidden(&metadata) {
                continue;
            }
            if metadata.is_symlink() || reparse_point(&metadata) {
                return Err(
                    tr("The folder contains a symbolic link. Remove it before publishing.", "A pasta contém um atalho simbólico. Remova-o antes de publicar.").into(),
                );
            }
            let key = format!("{prefix}{name}");
            if !valid_key(&key) {
                return Err(
                    tr("The folder contains a filename incompatible with URLs or S3.", "A pasta contém um nome de arquivo incompatível com URLs ou com o S3.").into(),
                );
            }
            if metadata.is_dir() {
                pending.push((path, format!("{key}/")));
            } else if metadata.is_file() {
                files.push((
                    path,
                    SiteFile {
                        key,
                        size: metadata.len(),
                        revision: local_revision(&metadata)?,
                    },
                ));
            } else {
                return Err(tr("The folder contains an item that is not a regular file.", "A pasta contém um item que não é um arquivo regular.").into());
            }
        }
    }
    if !files.iter().any(|(_, file)| file.key == "index.html") {
        return Err(tr("The folder must contain index.html at its root.", "A pasta precisa conter um arquivo index.html na raiz.").into());
    }
    Ok((root, files))
}

fn hidden(_metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return _metadata.file_attributes() & 0x2 != 0;
    }
    #[cfg(not(windows))]
    false
}

fn reparse_point(_metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return _metadata.file_attributes() & 0x400 != 0;
    }
    #[cfg(not(windows))]
    false
}

#[cfg(test)]
mod tests {
    use super::{collect_files, copy_source, remote_location, valid_key};
    use crate::models::AppConfig;

    #[test]
    fn scopes_remote_folders_and_encodes_object_names() {
        let config = AppConfig {
            bucket_name: "sync-bucket".into(),
            s3_prefix: Some("/web/".into()),
            ..AppConfig::default()
        };
        assert_eq!(
            remote_location(&config, "").unwrap(),
            ("sync-bucket", "web/".into())
        );
        assert_eq!(
            remote_location(&config, "web/site").unwrap(),
            ("sync-bucket", "web/web/site/".into())
        );
        assert!(remote_location(&config, "../other").is_err());
        assert!(!valid_key("site/.s3-drive-trash/index.html"));
        assert!(!valid_key("site//index.html"));
        assert_eq!(
            copy_source("sync-bucket", "web/a ç+?#%.svg"),
            "sync-bucket/web/a%20%C3%A7%2B%3F%23%25.svg"
        );
    }

    #[tokio::test]
    async fn includes_nested_assets_and_skips_dot_files() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        std::fs::write(root.join("index.html"), "<h1>Site</h1>").unwrap();
        std::fs::create_dir(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/style.css"), "h1 {}").unwrap();
        std::fs::write(root.join(".env"), "example").unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/config"), "example").unwrap();

        let (_, files) = collect_files(root).await.unwrap();
        let mut keys: Vec<_> = files.into_iter().map(|(_, file)| file.key).collect();
        keys.sort();
        assert_eq!(keys, ["assets/style.css", "index.html"]);
    }

    #[tokio::test]
    async fn requires_index_in_root() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::create_dir(folder.path().join("nested")).unwrap();
        std::fs::write(folder.path().join("nested/index.html"), "").unwrap();
        let error = collect_files(folder.path()).await.unwrap_err();
        assert!(error.contains("index.html at its root"));
    }
}
