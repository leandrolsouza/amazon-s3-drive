use crate::models::{AppConfig, ConfigError, ValidationError};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ConfigParser {
    path: PathBuf,
}

impl ConfigParser {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_location() -> Self {
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            path: base.join("amazon-s3-drive").join("config.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    pub fn load(&self) -> Result<AppConfig, ConfigError> {
        let bytes = fs::read(&self.path).map_err(|e| ConfigError::Io(e.to_string()))?;
        let content =
            String::from_utf8(bytes).map_err(|e| ConfigError::InvalidUtf8(e.to_string()))?;
        let mut config: AppConfig =
            serde_json::from_str(&content).map_err(|e| ConfigError::MalformedJson(e.to_string()))?;
        // Requisito 10.4: notificações de erro sempre habilitadas
        config.notifications.error_enabled = true;
        Ok(config)
    }

    pub fn save(&self, config: &AppConfig) -> Result<(), ConfigError> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| ConfigError::Io(e.to_string()))?;
            }
        }
        let mut normalized = config.clone();
        normalized.notifications.error_enabled = true;
        let json = serde_json::to_string_pretty(&normalized)
            .map_err(|e| ConfigError::MalformedJson(e.to_string()))?;
        fs::write(&self.path, json.as_bytes()).map_err(|e| ConfigError::Io(e.to_string()))?;
        Ok(())
    }

    pub fn create_empty(&self) -> Result<(), ConfigError> {
        let empty = AppConfig::default();
        self.save(&empty)
    }

    pub fn validate(config: &AppConfig) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();

        let bucket_trimmed = config.bucket_name.trim();
        if bucket_trimmed.is_empty() {
            errors.push(ValidationError {
                field: "bucket_name".to_string(),
                reason: "O nome do bucket S3 é obrigatório.".to_string(),
            });
        } else {
            let bucket_re = Regex::new(r"^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$").unwrap();
            if !bucket_re.is_match(bucket_trimmed) {
                errors.push(ValidationError {
                    field: "bucket_name".to_string(),
                    reason: "Nome de bucket S3 inválido (deve ter 3 a 63 caracteres minúsculos, números, pontos ou hífens).".to_string(),
                });
            }
        }

        if config.local_path.trim().is_empty() {
            errors.push(ValidationError {
                field: "local_path".to_string(),
                reason: "O caminho da pasta local é obrigatório.".to_string(),
            });
        }

        let region_trimmed = config.aws_region.trim();
        let has_custom_endpoint = config
            .endpoint_url
            .as_ref()
            .map(|u| !u.trim().is_empty())
            .unwrap_or(false);

        if region_trimmed.is_empty() {
            errors.push(ValidationError {
                field: "aws_region".to_string(),
                reason: "A região AWS é obrigatória.".to_string(),
            });
        } else if !has_custom_endpoint || region_trimmed != "auto" {
            let region_re = Regex::new(r"^[a-z]{2}-[a-z]+-\d$").unwrap();
            if !region_re.is_match(region_trimmed) {
                errors.push(ValidationError {
                    field: "aws_region".to_string(),
                    reason: "Formato de região AWS inválido (ex: us-east-1, sa-east-1).".to_string(),
                });
            }
        }

        if let Some(ref ep) = config.endpoint_url {
            let ep_trimmed = ep.trim();
            if !ep_trimmed.is_empty()
                && !ep_trimmed.starts_with("http://")
                && !ep_trimmed.starts_with("https://")
            {
                errors.push(ValidationError {
                    field: "endpoint_url".to_string(),
                    reason: "O Endpoint S3 customizado deve iniciar com http:// ou https://."
                        .to_string(),
                });
            }
        }

        if config.sync_interval_secs == 0 || config.sync_interval_secs > 60 {
            errors.push(ValidationError {
                field: "sync_interval_secs".to_string(),
                reason: "O intervalo de sincronização deve estar entre 1 e 60 segundos.".to_string(),
            });
        }

        // Requisito 9.6: validar sintaxe glob dos padrões de exclusão
        for pattern in &config.exclusion_patterns {
            if let Err(e) = glob::Pattern::new(pattern) {
                errors.push(ValidationError {
                    field: "exclusion_patterns".to_string(),
                    reason: format!("Padrão glob inválido '{}': {}", pattern, e),
                });
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::NotificationSettings;
    use proptest::prelude::*;
    use tempfile::tempdir;

    fn arb_notification_settings() -> impl Strategy<Value = NotificationSettings> {
        (any::<bool>(), any::<bool>(), any::<bool>()).prop_map(
            |(sync_complete_enabled, conflict_enabled, error_enabled)| NotificationSettings {
                sync_complete_enabled,
                conflict_enabled,
                error_enabled,
            },
        )
    }

    pub fn arb_app_config() -> impl Strategy<Value = AppConfig> {
        (
            "[a-z0-9]{3,20}",
            "[a-zA-Z0-9_/\\\\:-]{1,40}",
            prop::option::of("[a-zA-Z0-9/_-]{1,20}"),
            "(us|sa|eu|ap)-(east|west|south|north|central)-[1-9]",
            1u64..=60u64,
            prop::collection::vec("(\\*\\.[a-z]{2,4}|\\.[a-zA-Z0-9_]+|[a-z]+\\*)", 0..=8),
            arb_notification_settings(),
            any::<bool>(),
        )
            .prop_map(
                |(
                    bucket_name,
                    local_path,
                    s3_prefix,
                    aws_region,
                    sync_interval_secs,
                    exclusion_patterns,
                    notifications,
                    use_iam_role,
                )| {
                    AppConfig {
                        bucket_name,
                        local_path,
                        s3_prefix,
                        aws_region,
                        sync_interval_secs,
                        exclusion_patterns,
                        notifications,
                        use_iam_role,
                        endpoint_url: None,
                        force_path_style: false,
                        max_concurrent_transfers: 4,
                        storage_class: "STANDARD".to_string(),
                        server_side_encryption: false,
                        bandwidth_limit_kbps: 0,
                        launch_on_startup: false,
                        sync_direction: "Bidirectional".to_string(),
                    }
                },
            )
    }

    // Feature: amazon-s3-drive, Property 1: Config serialization round-trip
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn config_roundtrip(config in arb_app_config()) {
            let json = serde_json::to_string(&config).unwrap();
            let restored: AppConfig = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(config, restored);
        }
    }

    #[test]
    fn test_create_empty_generates_file_with_required_fields() {
        let dir = tempdir().unwrap();
        let cfg_path = dir.path().join("amazon-s3-drive").join("config.json");
        let parser = ConfigParser::new(cfg_path.clone());

        parser.create_empty().unwrap();
        assert!(cfg_path.exists());

        let raw = fs::read_to_string(&cfg_path).unwrap();
        assert!(raw.contains("\"bucket_name\""));
        assert!(raw.contains("\"local_path\""));
        assert!(raw.contains("\"aws_region\""));
        assert!(raw.contains("\"sync_interval_secs\""));
        assert!(raw.contains("\"exclusion_patterns\""));
        assert!(raw.contains("\"notifications\""));

        let loaded = parser.load().unwrap();
        assert_eq!(loaded, AppConfig::default());
    }

    #[test]
    fn test_load_returns_malformed_json_error() {
        let dir = tempdir().unwrap();
        let cfg_path = dir.path().join("config.json");
        fs::write(&cfg_path, b"{invalid}").unwrap();

        let parser = ConfigParser::new(cfg_path);
        let err = parser.load().unwrap_err();
        match err {
            ConfigError::MalformedJson(msg) => assert!(!msg.is_empty()),
            other => panic!("Expected MalformedJson, got {:?}", other),
        }
    }

    #[test]
    fn test_load_returns_invalid_utf8_error() {
        let dir = tempdir().unwrap();
        let cfg_path = dir.path().join("config.json");
        // Sequência de bytes inválida em UTF-8
        fs::write(&cfg_path, &[0xff, 0xfe, 0xfd]).unwrap();

        let parser = ConfigParser::new(cfg_path);
        let err = parser.load().unwrap_err();
        match err {
            ConfigError::InvalidUtf8(msg) => assert!(!msg.is_empty()),
            other => panic!("Expected InvalidUtf8, got {:?}", other),
        }
    }

    #[test]
    fn test_validate_invalid_glob_pattern() {
        let config = AppConfig {
            bucket_name: "my-valid-bucket".to_string(),
            local_path: "/tmp/s3drive".to_string(),
            s3_prefix: None,
            aws_region: "us-east-1".to_string(),
            sync_interval_secs: 60,
            exclusion_patterns: vec!["[unclosed".to_string()],
            notifications: NotificationSettings::default(),
            use_iam_role: false,
            endpoint_url: None,
            force_path_style: false,
            max_concurrent_transfers: 4,
            storage_class: "STANDARD".to_string(),
            server_side_encryption: false,
            bandwidth_limit_kbps: 0,
            launch_on_startup: false,
            sync_direction: "Bidirectional".to_string(),
        };
        let errs = ConfigParser::validate(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.field == "exclusion_patterns"));
    }
}
