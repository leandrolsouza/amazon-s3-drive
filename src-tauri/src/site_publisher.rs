use crate::language::tr;
pub mod sources;

use crate::{
    models::{AppConfig, AwsCredentials},
    AppState,
};
use aws_credential_types::Credentials;
use aws_sdk_cloudformation::{
    error::ProvideErrorMetadata,
    types::{Parameter, Stack, StackStatus, Tag},
    Client,
};
use aws_sdk_s3::primitives::ByteStream;
use serde::{Deserialize, Serialize};
use sources::{Selection, SiteFile, SiteSource};
use std::time::Duration;
use tauri::State;

const LEGACY_STACK: &str = "AmazonS3DriveStaticSite";
const STACK_PREFIX: &str = "S3DriveSite-";
const STACK_TAG: &str = "AmazonS3DriveStaticSiteVersion";
const TEMPLATE: &str = include_str!("site-stack.json");
const HISTORY_PREFIX: &str = "site-history/";

#[derive(Serialize)]
pub struct PublishedSite {
    id: String,
    bucket: String,
    url: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SiteDestination {
    New { bucket: String },
    Existing { id: String },
}

#[derive(Serialize)]
pub struct SiteRelease {
    id: String,
    timestamp_ms: Option<i64>,
    active: bool,
}

#[tauri::command]
pub async fn list_published_sites(
    state: State<'_, AppState>,
) -> Result<Vec<PublishedSite>, String> {
    let config = load_config(&state)?;
    let client = cloudformation_client(&sdk_config(&state, &config).await?);
    let mut sites = Vec::new();
    let mut pages = client.list_stacks().into_paginator().send();
    while let Some(page) = pages.next().await {
        let page =
            page.map_err(|error| aws_error(tr("list sites in CloudFormation", "listar os sites no CloudFormation"), error.code()))?;
        for summary in page.stack_summaries() {
            let Some(name) = summary.stack_name() else {
                continue;
            };
            if !is_site_name(name) || summary.stack_status() == Some(&StackStatus::DeleteComplete) {
                continue;
            }
            let Some(id) = summary.stack_id() else {
                continue;
            };
            let Some(stack) = describe_stack(&client, id).await? else {
                continue;
            };
            if is_managed(&stack) && stack_output(&stack, "Bucket").is_some() {
                sites.push(site_details(&stack)?);
            }
        }
    }
    sites.sort_by(|a, b| a.bucket.cmp(&b.bucket));
    Ok(sites)
}

#[tauri::command]
pub async fn publish_site(
    source: SiteSource,
    files: Vec<SiteFile>,
    destination: SiteDestination,
    state: State<'_, AppState>,
) -> Result<PublishedSite, String> {
    let _publication = state
        .site_publication
        .try_lock()
        .map_err(|_| tr("Publishing is already in progress.", "Já existe uma publicação em andamento."))?;
    let config = load_config(&state)?;
    let sdk = sdk_config(&state, &config).await?;
    let cloudformation = cloudformation_client(&sdk);
    let s3 = s3_client(&sdk);
    let selected = Selection::validate(source, files, &config, &s3).await?;
    let stack = match destination {
        SiteDestination::New { bucket } => {
            if !valid_bucket_name(&bucket) {
                return Err(tr("Use 3 to 63 lowercase letters, numbers, and hyphens in the bucket name. Start and end with a letter or number and avoid S3 reserved prefixes and suffixes.", "Use de 3 a 63 letras minúsculas, números e hífens no nome do bucket. Comece e termine com letra ou número e evite prefixos e sufixos reservados pelo S3.").into());
            }
            if bucket == config.bucket_name.trim() {
                return Err(tr("Choose a name different from the sync bucket name.", "Escolha um nome diferente do bucket de sincronização.").into());
            }
            create_or_resume_site(&cloudformation, &s3, &bucket).await?
        }
        SiteDestination::Existing { id } => require_site(&cloudformation, &id).await?,
    };
    ensure_writable(&stack)?;
    let bucket = stack_output(&stack, "Bucket").ok_or(tr("The publishing bucket is unavailable.", "Bucket de publicação indisponível."))?;
    if bucket == config.bucket_name.trim() {
        return Err(tr("Choose a destination separate from the sync bucket.", "Escolha um destino separado do bucket de sincronização.").into());
    }
    let release = format!("{:032x}", rand::random::<u128>());
    if let Some(previous) = active_release(&stack) {
        mark_release(&s3, bucket, previous).await?;
    }
    selected.upload(&s3, bucket, &release).await?;
    mark_release(&s3, bucket, &release).await?;
    site_details(&activate_release(&cloudformation, &stack, &release).await?)
}

#[tauri::command]
pub async fn list_site_releases(
    id: String,
    state: State<'_, AppState>,
) -> Result<Vec<SiteRelease>, String> {
    let config = load_config(&state)?;
    let sdk = sdk_config(&state, &config).await?;
    let stack = require_site(&cloudformation_client(&sdk), &id).await?;
    let bucket = stack_output(&stack, "Bucket").ok_or(tr("The publishing bucket is unavailable.", "Bucket de publicação indisponível."))?;
    let active = active_release(&stack);
    let s3 = s3_client(&sdk);
    let mut pages = s3
        .list_objects_v2()
        .bucket(bucket)
        .prefix(HISTORY_PREFIX)
        .into_paginator()
        .send();
    let mut releases = Vec::new();
    while let Some(page) = pages.next().await {
        let page = page.map_err(|error| aws_error(tr("list site releases", "listar as versões do site"), error.code()))?;
        for object in page.contents() {
            let Some(id) = object
                .key()
                .and_then(|key| key.strip_prefix(HISTORY_PREFIX))
                .filter(|id| valid_release(id))
            else {
                continue;
            };
            releases.push(SiteRelease {
                id: id.to_owned(),
                timestamp_ms: object
                    .last_modified()
                    .and_then(|date| date.to_millis().ok()),
                active: active == Some(id),
            });
        }
    }
    if let Some(id) = active.filter(|id| !releases.iter().any(|release| release.id == *id)) {
        releases.push(SiteRelease {
            id: id.to_owned(),
            timestamp_ms: None,
            active: true,
        });
    }
    releases.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then_with(|| b.timestamp_ms.cmp(&a.timestamp_ms))
    });
    Ok(releases)
}

#[tauri::command]
pub async fn restore_site_release(
    id: String,
    release: String,
    state: State<'_, AppState>,
) -> Result<PublishedSite, String> {
    let _publication = state
        .site_publication
        .try_lock()
        .map_err(|_| tr("Publishing is already in progress.", "Já existe uma publicação em andamento."))?;
    if !valid_release(&release) {
        return Err(tr("Invalid site release.", "Versão do site inválida.").into());
    }
    let config = load_config(&state)?;
    let sdk = sdk_config(&state, &config).await?;
    let client = cloudformation_client(&sdk);
    let stack = require_site(&client, &id).await?;
    ensure_writable(&stack)?;
    if active_release(&stack) == Some(&release) {
        return site_details(&stack);
    }
    let bucket = stack_output(&stack, "Bucket").ok_or(tr("The publishing bucket is unavailable.", "Bucket de publicação indisponível."))?;
    let s3 = s3_client(&sdk);
    for key in [
        format!("{HISTORY_PREFIX}{release}"),
        format!("releases/{release}/index.html"),
    ] {
        s3.head_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| aws_error(tr("verify the release files", "verificar os arquivos da versão"), error.code()))?;
    }
    site_details(&activate_release(&client, &stack, &release).await?)
}

async fn create_or_resume_site(
    client: &Client,
    s3: &aws_sdk_s3::Client,
    bucket: &str,
) -> Result<Stack, String> {
    let name = format!("{STACK_PREFIX}{bucket}");
    if let Some(stack) = find_stack(client, &name).await? {
        ensure_writable(&stack)?;
        if active_release(&stack).is_some() {
            return Err(tr("A site already uses this bucket. Select the site to republish it or choose another name.", "Já existe um site com esse bucket. Selecione o site para republicar ou escolha outro nome.").into());
        }
        return Ok(stack);
    }
    match s3.head_bucket().bucket(bucket).send().await {
        Err(error)
            if error
                .raw_response()
                .is_some_and(|response| response.status().as_u16() == 404) => {}
        Ok(_) => return Err(tr("This bucket name is already in use. Choose another name.", "Esse nome de bucket já está em uso. Escolha outro nome.").into()),
        Err(error)
            if error
                .raw_response()
                .is_some_and(|response| matches!(response.status().as_u16(), 301 | 400 | 403)) =>
        {
            return Err(tr("The bucket name is in use or AWS blocked the check. Try another name or check your permissions.", "O nome do bucket está em uso ou a AWS impediu a verificação. Tente outro nome ou confira suas permissões.").into());
        }
        Err(error) => return Err(aws_error(tr("check the bucket name", "verificar o nome do bucket"), error.code())),
    }
    let created = client
        .create_stack()
        .stack_name(&name)
        .template_body(TEMPLATE)
        .parameters(
            Parameter::builder()
                .parameter_key("BucketName")
                .parameter_value(bucket)
                .build(),
        )
        .tags(Tag::builder().key(STACK_TAG).value("1").build())
        .send()
        .await
        .map_err(|error| aws_error(tr("create the site infrastructure", "criar a infraestrutura do site"), error.code()))?;
    let id = created
        .stack_id()
        .ok_or(tr("Site identifier unavailable.", "Identificador do site indisponível."))?;
    wait_for_stack(client, id, None).await
}

async fn activate_release(client: &Client, stack: &Stack, release: &str) -> Result<Stack, String> {
    let id = stack
        .stack_id()
        .ok_or(tr("Site identifier unavailable.", "Identificador do site indisponível."))?;
    let mut update = client
        .update_stack()
        .stack_name(id)
        .use_previous_template(true)
        .parameters(
            Parameter::builder()
                .parameter_key("Release")
                .parameter_value(release)
                .build(),
        );
    if stack
        .parameters()
        .iter()
        .any(|parameter| parameter.parameter_key() == Some("BucketName"))
    {
        update = update.parameters(
            Parameter::builder()
                .parameter_key("BucketName")
                .use_previous_value(true)
                .build(),
        );
    }
    update
        .send()
        .await
        .map_err(|error| aws_error(tr("activate the site release", "ativar a versão do site"), error.code()))?;
    wait_for_stack(client, id, Some((stack, release))).await
}

async fn mark_release(s3: &aws_sdk_s3::Client, bucket: &str, release: &str) -> Result<(), String> {
    match s3
        .put_object()
        .bucket(bucket)
        .key(format!("{HISTORY_PREFIX}{release}"))
        .if_none_match("*")
        .body(ByteStream::from_static(b""))
        .send()
        .await
    {
        Ok(_) => Ok(()),
        Err(error) if error.code() == Some("PreconditionFailed") => Ok(()),
        Err(error) => Err(aws_error(tr("register the site release", "registrar a versão do site"), error.code())),
    }
}

fn active_release(stack: &Stack) -> Option<&str> {
    stack
        .parameters()
        .iter()
        .find(|parameter| parameter.parameter_key() == Some("Release"))
        .and_then(|parameter| parameter.parameter_value())
        .filter(|id| valid_release(id))
}

fn valid_release(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_bucket_name(name: &str) -> bool {
    let alphanumeric = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    (3..=63).contains(&name.len())
        && name.bytes().all(|byte| alphanumeric(byte) || byte == b'-')
        && name.bytes().next().is_some_and(alphanumeric)
        && name.bytes().last().is_some_and(alphanumeric)
        && !["xn--", "sthree-", "amzn-s3-demo-"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
        && !["-s3alias", "--ol-s3", "--x-s3", "--table-s3", "-an"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

fn is_site_name(name: &str) -> bool {
    name == LEGACY_STACK
        || name
            .strip_prefix(STACK_PREFIX)
            .is_some_and(valid_bucket_name)
}

fn is_managed(stack: &Stack) -> bool {
    stack
        .tags()
        .iter()
        .any(|tag| tag.key() == Some(STACK_TAG) && tag.value() == Some("1"))
}

fn site_details(stack: &Stack) -> Result<PublishedSite, String> {
    Ok(PublishedSite {
        id: stack
            .stack_id()
            .ok_or(tr("Site identifier unavailable.", "Identificador do site indisponível."))?
            .to_owned(),
        bucket: stack_output(stack, "Bucket")
            .ok_or(tr("The publishing bucket is unavailable.", "Bucket de publicação indisponível."))?
            .to_owned(),
        url: stack_output(stack, "SiteUrl").map(str::to_owned),
    })
}

async fn require_site(client: &Client, id: &str) -> Result<Stack, String> {
    let name = id
        .strip_prefix("arn:")
        .and_then(|arn| arn.split_once(":cloudformation:"))
        .and_then(|(_, arn)| arn.split_once(":stack/"))
        .and_then(|(_, resource)| resource.split_once('/'))
        .map(|(name, _)| name);
    if !name.is_some_and(is_site_name) {
        return Err(tr("Invalid site.", "Site inválido.").into());
    }
    find_stack(client, id)
        .await?
        .ok_or_else(|| tr("The site is unavailable in the configured account and region.", "Site indisponível na conta e região configuradas.").into())
}

fn ensure_writable(stack: &Stack) -> Result<(), String> {
    match stack.stack_status() {
        Some(StackStatus::CreateComplete | StackStatus::UpdateComplete | StackStatus::UpdateRollbackComplete) => Ok(()),
        Some(status) if status.as_str().ends_with("_IN_PROGRESS") => Err(tr("AWS is processing a site change. Try again when the operation finishes.", "A AWS está processando uma alteração no site. Tente novamente quando a operação terminar.").into()),
        _ => Err(stack_failure(stack)),
    }
}

fn load_config(state: &AppState) -> Result<AppConfig, String> {
    state
        .config_parser
        .load()
        .map_err(|_| tr("Save your AWS settings first.", "Salve a configuração AWS primeiro.").into())
}

fn s3_client(sdk: &aws_config::SdkConfig) -> aws_sdk_s3::Client {
    let mut config = aws_sdk_s3::config::Builder::from(sdk);
    config.set_endpoint_url(None);
    aws_sdk_s3::Client::from_conf(config.build())
}

async fn sdk_config(
    state: &AppState,
    config: &crate::models::AppConfig,
) -> Result<aws_config::SdkConfig, String> {
    if config.aws_region.trim().is_empty() {
        return Err(tr("Set the AWS region before publishing.", "Configure a região AWS antes de publicar.").into());
    }
    if config
        .endpoint_url
        .as_deref()
        .is_some_and(|url| !url.trim().is_empty())
    {
        return Err(tr("Publishing requires a direct AWS connection.", "A publicação requer uma conexão direta com a AWS.").into());
    }
    let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(config.aws_region.trim().to_owned()));
    if !config.use_iam_role {
        let AwsCredentials {
            access_key_id,
            secret_access_key,
            session_token,
            ..
        } = state
            .creds_manager
            .load()
            .map_err(|_| tr("Could not access AWS credentials.", "Não foi possível acessar as credenciais AWS."))?
            .ok_or(tr("Configure AWS credentials before publishing.", "Configure as credenciais AWS antes de publicar."))?;
        loader = loader.credentials_provider(Credentials::new(
            access_key_id,
            secret_access_key,
            session_token,
            None,
            "amazon-s3-drive",
        ));
    }
    Ok(loader.load().await)
}

fn cloudformation_client(sdk: &aws_config::SdkConfig) -> Client {
    let mut config = aws_sdk_cloudformation::config::Builder::from(sdk);
    config.set_endpoint_url(None);
    Client::from_conf(config.build())
}

async fn describe_stack(client: &Client, name: &str) -> Result<Option<Stack>, String> {
    let result = client.describe_stacks().stack_name(name).send().await;
    let stack = match result {
        Ok(response) => response.stacks.into_iter().flatten().next(),
        Err(error)
            if error.code() == Some("ValidationError")
                && error
                    .message()
                    .is_some_and(|message| message.ends_with("does not exist")) =>
        {
            None
        }
        Err(error) => {
            return Err(aws_error(
                tr("look up the site in CloudFormation", "consultar o site no CloudFormation"),
                error.code(),
            ))
        }
    };
    Ok(stack)
}

async fn find_stack(client: &Client, name: &str) -> Result<Option<Stack>, String> {
    let stack = describe_stack(client, name).await?;
    if stack.as_ref().is_some_and(|stack| !is_managed(stack)) {
        return Err(tr("The selected stack does not belong to Amazon S3 Drive.", "A pilha selecionada não pertence ao Amazon S3 Drive.").into());
    }
    Ok(stack)
}

async fn wait_for_stack(
    client: &Client,
    name: &str,
    update: Option<(&Stack, &str)>,
) -> Result<Stack, String> {
    tokio::time::timeout(Duration::from_secs(45 * 60), async {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let Some(stack) = find_stack(client, name).await? else {
                continue;
            };
            let status = stack.stack_status().map(|status| status.as_str()).unwrap_or("");
            if let Some((previous, release)) = update {
                if status == "UPDATE_COMPLETE" && active_release(&stack) == Some(release) {
                    return Ok(stack);
                }
                if stack.last_updated_time() == previous.last_updated_time()
                    && stack.stack_status() == previous.stack_status()
                {
                    continue;
                }
            } else if status == "CREATE_COMPLETE" {
                return Ok(stack);
            }
            if !status.ends_with("_IN_PROGRESS") {
                return Err(stack_failure(&stack));
            }
        }
    })
    .await
    .map_err(|_| tr("AWS may still be publishing. Check the site status before trying again.", "A AWS ainda pode estar processando a publicação. Consulte o estado do site antes de tentar novamente."))?
}

fn stack_output<'a>(stack: &'a Stack, key: &str) -> Option<&'a str> {
    stack
        .outputs()
        .iter()
        .find(|output| output.output_key() == Some(key))
        .and_then(|output| output.output_value())
}

fn stack_failure(stack: &Stack) -> String {
    let status = stack
        .stack_status()
        .map(|status| status.as_str())
        .unwrap_or(tr("unknown", "desconhecido"));
    crate::language::localized_format!(
        "Stack {} is in {status}. Check the events in CloudFormation.", "A pilha {} está em {status}. Consulte os eventos no CloudFormation.",
        stack.stack_name().unwrap_or(tr("(site)", "do site"))
    )
}

fn aws_error(action: &str, code: Option<&str>) -> String {
    crate::language::localized_format!(
        "Could not {action} ({}). Check the connection and AWS permissions.", "Não foi possível {action} ({}). Verifique a conexão e as permissões AWS.",
        code.unwrap_or(tr("no response from AWS", "sem resposta da AWS"))
    )
}

#[cfg(test)]
mod tests {
    use super::{is_site_name, valid_bucket_name};

    #[test]
    fn accepts_site_names_and_preserves_the_legacy_stack() {
        assert!(valid_bucket_name("pagina-comercial-7k2m"));
        assert!(is_site_name("S3DriveSite-pagina-comercial"));
        assert!(is_site_name("AmazonS3DriveStaticSite"));
        assert!(!is_site_name("UnrelatedStack"));
        for name in [
            "a",
            "-pagina",
            "pagina-",
            "Página",
            "xn--pagina",
            "pagina--x-s3",
        ] {
            assert!(!valid_bucket_name(name));
        }
    }
}
