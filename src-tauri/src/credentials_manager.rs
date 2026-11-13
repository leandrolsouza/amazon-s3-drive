use crate::language::tr;
use crate::models::{AwsCredentials, CredError, ValidationError};
use crate::s3_client::S3Client;
use regex::Regex;

const KEYRING_SERVICE: &str = "amazon-s3-drive";
const KEYRING_USER: &str = "aws-credentials";

#[derive(Debug, Clone, Default)]
pub struct CredentialsManager;

impl CredentialsManager {
    pub fn new() -> Self {
        Self
    }

    fn entry() -> Result<keyring::Entry, CredError> {
        keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
            .map_err(|_| CredError::Keyring(tr("The system keyring is unavailable.", "Cofre de credenciais indisponível.").into()))
    }

    pub fn store(&self, creds: &AwsCredentials) -> Result<(), CredError> {
        let json =
            serde_json::to_string(creds).map_err(|e| CredError::Serialization(e.to_string()))?;
        Self::entry()?.set_password(&json).map_err(|_| {
            CredError::Keyring(tr("Could not save credentials to the system keyring.", "Não foi possível salvar as credenciais no cofre.").into())
        })
    }

    pub fn load(&self) -> Result<Option<AwsCredentials>, CredError> {
        match Self::entry()?.get_password() {
            Ok(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(|_| CredError::Serialization(tr("The stored credentials are invalid.", "Credenciais armazenadas inválidas.").into())),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(CredError::Keyring(
                tr("Could not read credentials from the system keyring.", "Não foi possível ler as credenciais do cofre.").into(),
            )),
        }
    }

    pub fn delete(&self) -> Result<(), CredError> {
        match Self::entry()?.delete_password() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(CredError::Keyring(
                tr("Could not remove credentials from the system keyring.", "Não foi possível remover as credenciais do cofre.").into(),
            )),
        }
    }

    /// Valida o formato dos campos sem chamar a API AWS (Requisito 4.1 e 4.2)
    pub fn validate_format(creds: &AwsCredentials) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();

        if !creds.use_iam_role {
            let ak_re = Regex::new(r"^[A-Z0-9]{20}$").unwrap();
            if !ak_re.is_match(&creds.access_key_id) {
                errors.push(ValidationError {
                    field: "access_key_id".to_string(),
                    reason:
                        tr("Access Key ID must contain exactly 20 uppercase alphanumeric characters.", "Access Key ID deve conter exatamente 20 caracteres alfanuméricos maiúsculos.")
                            .to_string(),
                });
            }

            if creds.secret_access_key.chars().count() != 40 {
                errors.push(ValidationError {
                    field: "secret_access_key".to_string(),
                    reason: tr("Secret Access Key must contain exactly 40 characters.", "Secret Access Key deve conter exatamente 40 caracteres.").to_string(),
                });
            }
        }

        let region_re = Regex::new(r"^[a-z]{2}-[a-z]+-\d$").unwrap();
        if !region_re.is_match(&creds.region) {
            errors.push(ValidationError {
                field: "region".to_string(),
                reason: tr("Invalid AWS region format (e.g. us-east-1, sa-east-1).", "AWS Region inválida (deve seguir o formato ex: us-east-1, sa-east-1).")
                    .to_string(),
            });
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Verifica acesso ao bucket realizando uma chamada de teste à API S3 (Requisito 4.6 e 4.7)
    pub async fn verify_access(&self, s3_client: &S3Client) -> Result<(), CredError> {
        s3_client
            .test_access()
            .await
            .map_err(|e| CredError::AccessDenied(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn reports_unavailable_keyring_on_read_and_delete() {
        struct UnavailableKeyring;

        impl keyring::credential::CredentialBuilderApi for UnavailableKeyring {
            fn build(
                &self,
                _: Option<&str>,
                _: &str,
                _: &str,
            ) -> keyring::Result<Box<keyring::Credential>> {
                Err(keyring::Error::NoStorageAccess(Box::new(
                    std::io::Error::from(std::io::ErrorKind::PermissionDenied),
                )))
            }

            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
        }

        keyring::set_default_credential_builder(Box::new(UnavailableKeyring));
        let manager = CredentialsManager::new();
        assert!(matches!(manager.load(), Err(CredError::Keyring(_))));
        assert!(matches!(manager.delete(), Err(CredError::Keyring(_))));
    }

    fn arb_valid_credentials() -> impl Strategy<Value = AwsCredentials> {
        (
            "[A-Z0-9]{20}",
            "[A-Za-z0-9/+]{40}",
            prop::option::of("[A-Za-z0-9/+=]{16,32}"),
            "[a-z]{2}-[a-z]{3,10}-[0-9]",
        )
            .prop_map(
                |(access_key_id, secret_access_key, session_token, region)| AwsCredentials {
                    access_key_id,
                    secret_access_key,
                    session_token,
                    region,
                    use_iam_role: false,
                },
            )
    }

    fn arb_invalid_credentials() -> impl Strategy<Value = AwsCredentials> {
        prop_oneof![
            // access_key_id inválido (tamanho != 20 ou minúsculas)
            (
                "[a-z0-9]{1,19}",
                "[A-Za-z0-9/+]{40}",
                "[a-z]{2}-[a-z]{4}-[1-9]"
            ),
            // secret_access_key inválido (tamanho != 40)
            (
                "[A-Z0-9]{20}",
                "[A-Za-z0-9/+]{1,39}",
                "[a-z]{2}-[a-z]{4}-[1-9]"
            ),
            // region inválida
            (
                "[A-Z0-9]{20}",
                "[A-Za-z0-9/+]{40}",
                "[A-Z]{2}-[A-Z]{4}-[1-9]"
            ),
        ]
        .prop_map(
            |(access_key_id, secret_access_key, region)| AwsCredentials {
                access_key_id,
                secret_access_key,
                session_token: None,
                region,
                use_iam_role: false,
            },
        )
    }

    // Feature: amazon-s3-drive, Property 3: Credential format validation
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn valid_creds_accepted(creds in arb_valid_credentials()) {
            prop_assert!(CredentialsManager::validate_format(&creds).is_ok());
        }

        #[test]
        fn invalid_creds_identify_field(creds in arb_invalid_credentials()) {
            let errs = CredentialsManager::validate_format(&creds).unwrap_err();
            prop_assert!(!errs.is_empty());
            for err in &errs {
                prop_assert!(["access_key_id", "secret_access_key", "region"]
                    .contains(&err.field.as_str()));
                prop_assert!(!err.reason.is_empty());
            }
        }
    }

    #[test]
    fn test_access_key_id_length_19_and_21_rejected() {
        let creds_19 = AwsCredentials {
            access_key_id: "ABCDEFGHIJKLMNOPQRS".to_string(), // 19
            secret_access_key: "a".repeat(40),
            session_token: None,
            region: "us-east-1".to_string(),
            use_iam_role: false,
        };
        let err19 = CredentialsManager::validate_format(&creds_19).unwrap_err();
        assert_eq!(err19.len(), 1);
        assert_eq!(err19[0].field, "access_key_id");

        let creds_21 = AwsCredentials {
            access_key_id: "ABCDEFGHIJKLMNOPQRSTU".to_string(), // 21
            secret_access_key: "a".repeat(40),
            session_token: None,
            region: "us-east-1".to_string(),
            use_iam_role: false,
        };
        let err21 = CredentialsManager::validate_format(&creds_21).unwrap_err();
        assert_eq!(err21.len(), 1);
        assert_eq!(err21[0].field, "access_key_id");
    }

    #[test]
    fn test_secret_access_key_length_39_and_41_rejected() {
        let creds_39 = AwsCredentials {
            access_key_id: "A".repeat(20),
            secret_access_key: "s".repeat(39),
            session_token: None,
            region: "us-east-1".to_string(),
            use_iam_role: false,
        };
        let err39 = CredentialsManager::validate_format(&creds_39).unwrap_err();
        assert_eq!(err39.len(), 1);
        assert_eq!(err39[0].field, "secret_access_key");

        let creds_41 = AwsCredentials {
            access_key_id: "A".repeat(20),
            secret_access_key: "s".repeat(41),
            session_token: None,
            region: "us-east-1".to_string(),
            use_iam_role: false,
        };
        let err41 = CredentialsManager::validate_format(&creds_41).unwrap_err();
        assert_eq!(err41.len(), 1);
        assert_eq!(err41[0].field, "secret_access_key");
    }

    #[test]
    fn test_region_validation() {
        let valid = AwsCredentials {
            access_key_id: "A".repeat(20),
            secret_access_key: "s".repeat(40),
            session_token: None,
            region: "us-east-1".to_string(),
            use_iam_role: false,
        };
        assert!(CredentialsManager::validate_format(&valid).is_ok());

        let uppercase_region = AwsCredentials {
            region: "US-EAST-1".to_string(),
            ..valid.clone()
        };
        let err_upper = CredentialsManager::validate_format(&uppercase_region).unwrap_err();
        assert_eq!(err_upper[0].field, "region");

        let incomplete_region = AwsCredentials {
            region: "us-east".to_string(),
            ..valid
        };
        let err_inc = CredentialsManager::validate_format(&incomplete_region).unwrap_err();
        assert_eq!(err_inc[0].field, "region");
    }
}
