use keyring::Entry;
use uuid::Uuid;

const SERVICE: &str = "com.nexetry.e-sh";

#[derive(Debug, Clone, Copy)]
pub enum SecretKind {
    Password,
    Passphrase,
}

impl SecretKind {
    fn tag(self) -> &'static str {
        match self {
            SecretKind::Password => "password",
            SecretKind::Passphrase => "passphrase",
        }
    }
}

fn entry(kind: SecretKind, conn_id: Uuid) -> Result<Entry, keyring::Error> {
    let user = format!("{}:{}", kind.tag(), conn_id);
    Entry::new(SERVICE, &user)
}

pub fn store(kind: SecretKind, conn_id: Uuid, secret: &str) -> Result<(), keyring::Error> {
    let entry = entry(kind, conn_id)?;
    entry.set_password(secret)
}

pub fn fetch(kind: SecretKind, conn_id: Uuid) -> Option<String> {
    let entry = match entry(kind, conn_id) {
        Ok(e) => e,
        Err(err) => {
            tracing::debug!(?err, "keyring entry construction failed");
            return None;
        }
    };
    match entry.get_password() {
        Ok(value) => Some(value),
        Err(keyring::Error::NoEntry) => None,
        Err(err) => {
            tracing::debug!(?err, "keyring fetch failed");
            None
        }
    }
}

pub fn forget(kind: SecretKind, conn_id: Uuid) {
    let entry = match entry(kind, conn_id) {
        Ok(e) => e,
        Err(err) => {
            tracing::debug!(?err, "keyring entry construction failed");
            return;
        }
    };
    match entry.delete_credential() {
        Ok(_) | Err(keyring::Error::NoEntry) => {}
        Err(err) => tracing::debug!(?err, "keyring delete failed"),
    }
}
