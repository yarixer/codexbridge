use keyring::{Entry, Error};
use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;

const SERVICE: &str = "dev.yaro.yarocursor";
const CURSOR_ACCOUNT: &str = "cursor-api-key";

pub struct CursorCredentialStore {
    entry: Entry,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialStatus {
    pub configured: bool,
    pub masked: Option<String>,
    pub backend: &'static str,
    pub error: Option<String>,
}

impl CursorCredentialStore {
    pub fn new() -> Result<Self, String> {
        Entry::new(SERVICE, CURSOR_ACCOUNT)
            .map(|entry| Self { entry })
            .map_err(|error| format!("Системное хранилище секретов недоступно: {error}"))
    }

    pub fn load(&self) -> Result<Option<SecretString>, String> {
        match self.entry.get_password() {
            Ok(value) if value.trim().is_empty() => Ok(None),
            Ok(value) => Ok(Some(SecretString::from(value))),
            Err(Error::NoEntry) => Ok(None),
            Err(error) => Err(format!("Не удалось прочитать Cursor API key: {error}")),
        }
    }

    pub fn save(&self, value: &str) -> Result<(), String> {
        self.entry
            .set_password(value)
            .map_err(|error| format!("Не удалось сохранить Cursor API key: {error}"))
    }

    pub fn delete(&self) -> Result<(), String> {
        match self.entry.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(error) => Err(format!("Не удалось удалить Cursor API key: {error}")),
        }
    }
}

pub fn status(secret: Option<&SecretString>, error: Option<String>) -> CredentialStatus {
    CredentialStatus {
        configured: secret.is_some(),
        masked: secret.map(|secret| mask(secret.expose_secret())),
        backend: if cfg!(target_os = "windows") {
            "Windows Credential Manager"
        } else if cfg!(target_os = "linux") {
            "Secret Service"
        } else {
            "System credential store"
        },
        error,
    }
}

fn mask(value: &str) -> String {
    let characters = value.chars().collect::<Vec<_>>();
    if characters.len() <= 8 {
        return "••••••••".into();
    }
    let prefix = characters.iter().take(4).collect::<String>();
    let suffix = characters
        .iter()
        .skip(characters.len() - 4)
        .collect::<String>();
    format!("{prefix}••••{suffix}")
}

#[cfg(test)]
mod tests {
    use super::mask;

    #[test]
    fn masks_secret_without_exposing_the_middle() {
        assert_eq!(mask("crsr_0123456789"), "crsr••••6789");
        assert_eq!(mask("short"), "••••••••");
    }
}
