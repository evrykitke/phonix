//! The `mail_settings` row: one workspace's own relay, if it has one.
//!
//! One row per tenant database, created empty by migration 0007. Empty is a
//! meaningful state - it is what "this workspace uses the system default" looks
//! like - so the row is seeded by the migration rather than created on first
//! save, and every read returns something.
//!
//! # The password does not pass through this module in clear
//!
//! [`MailRow::password_sealed`] is bytes, and this module neither seals nor
//! opens them: that needs the vault key, which lives in the service layer with
//! the rest of the crypto. A repository that could decrypt would be a
//! repository that has to be trusted with the key, and the reason the sealed
//! form exists is so that reading rows is not the same as reading secrets.

use chrono::{DateTime, Utc};
use phonix_core::identity::UserId;
use phonix_core::mail::{MailEncryption, MailSettings};
use sqlx::{FromRow, PgExecutor, Row};

use crate::error::DbError;

/// The stored row, sealed password and all.
#[derive(Debug, Clone)]
pub struct MailRow {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// Nonce followed by ciphertext, as written by the service's vault. `None`
    /// when no password has been set, which is not the same as an empty one.
    pub password_sealed: Option<Vec<u8>>,
    pub from_address: String,
    pub from_name: String,
    pub reply_to: Option<String>,
    pub encryption: MailEncryption,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<UserId>,
}

impl MailRow {
    /// The part a screen is allowed to see.
    ///
    /// Note what this cannot do: [`MailSettings`] has no password field, so
    /// there is no version of this function that leaks one.
    pub fn to_settings(&self) -> MailSettings {
        MailSettings {
            enabled: self.enabled,
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            from_address: self.from_address.clone(),
            from_name: self.from_name.clone(),
            reply_to: self.reply_to.clone(),
            encryption: self.encryption,
            has_password: self.password_sealed.is_some(),
        }
    }
}

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for MailRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let port: i32 = row.try_get("port")?;
        let raw_encryption: String = row.try_get("encryption")?;

        // Refused rather than defaulted. The CHECK constraint means this can
        // only happen if the two ever disagree about the vocabulary, and a
        // silent fallback to StartTls would be a downgrade nobody was told
        // about.
        let encryption =
            MailEncryption::parse(&raw_encryption).ok_or_else(|| sqlx::Error::ColumnDecode {
                index: "encryption".to_owned(),
                source: format!("unrecognised mail encryption '{raw_encryption}'").into(),
            })?;

        Ok(Self {
            enabled: row.try_get("enabled")?,
            host: row.try_get("host")?,
            port: u16::try_from(port).map_err(|_| sqlx::Error::ColumnDecode {
                index: "port".to_owned(),
                source: format!("port {port} is outside the range of a port number").into(),
            })?,
            username: row.try_get("username")?,
            password_sealed: row.try_get("password_sealed")?,
            from_address: row.try_get("from_address")?,
            from_name: row.try_get("from_name")?,
            reply_to: row.try_get("reply_to")?,
            encryption,
            updated_at: row.try_get("updated_at")?,
            updated_by: row.try_get("updated_by")?,
        })
    }
}

const SELECT: &str = "SELECT enabled, host, port, username, password_sealed, from_address, \
     from_name, reply_to, encryption, updated_at, updated_by \
     FROM mail_settings WHERE id";

/// This workspace's relay row. Always present - the migration seeds it.
pub async fn load<'e, E>(executor: E) -> Result<MailRow, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, MailRow>(SELECT)
        .fetch_one(executor)
        .await
        .map_err(DbError::Query)
}

/// What a save writes.
///
/// `password_sealed` is `None` for "leave whatever is stored alone", which is
/// what lets the settings screen save a changed host without being handed the
/// password first. Clearing it is [`clear_password`], a separate call, because
/// "leave it" and "remove it" must not be the same value.
#[derive(Debug, Clone)]
pub struct MailUpdate<'a> {
    pub enabled: bool,
    pub host: &'a str,
    pub port: u16,
    pub username: &'a str,
    pub password_sealed: Option<&'a [u8]>,
    pub from_address: &'a str,
    pub from_name: &'a str,
    pub reply_to: Option<&'a str>,
    pub encryption: MailEncryption,
    pub updated_by: Option<UserId>,
}

/// Replace the row.
pub async fn save<'e, E>(executor: E, update: MailUpdate<'_>) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE mail_settings
            SET enabled         = $1,
                host            = $2,
                port            = $3,
                username        = $4,
                -- COALESCE, so a save that carries no password keeps the one
                -- already stored instead of wiping it.
                password_sealed = COALESCE($5, password_sealed),
                from_address    = $6,
                from_name       = $7,
                reply_to        = $8,
                encryption      = $9,
                updated_at      = now(),
                updated_by      = $10
          WHERE id",
    )
    .bind(update.enabled)
    .bind(update.host)
    .bind(i32::from(update.port))
    .bind(update.username)
    .bind(update.password_sealed)
    .bind(update.from_address)
    .bind(update.from_name)
    .bind(update.reply_to)
    .bind(update.encryption.as_str())
    .bind(update.updated_by)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Remove the stored password.
///
/// Its own statement rather than a `None` passed to [`save`], because there the
/// absence of a password means "unchanged" - and one value cannot mean both.
pub async fn clear_password<'e, E>(executor: E) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query("UPDATE mail_settings SET password_sealed = NULL, updated_at = now() WHERE id")
        .execute(executor)
        .await
        .map_err(DbError::Query)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> MailRow {
        MailRow {
            enabled: true,
            host: "smtp.example.com".into(),
            port: 587,
            username: "postmaster".into(),
            password_sealed: Some(vec![1, 2, 3]),
            from_address: "no-reply@example.com".into(),
            from_name: "Example".into(),
            reply_to: None,
            encryption: MailEncryption::StartTls,
            updated_at: Utc::now(),
            updated_by: None,
        }
    }

    #[test]
    fn the_screens_view_reports_that_a_password_exists_without_carrying_it() {
        let settings = row().to_settings();

        assert!(settings.has_password);

        let json = serde_json::to_string(&settings).unwrap();
        assert!(
            !json.contains("password_sealed"),
            "the sealed value reached the client: {json}"
        );
        assert!(!json.contains("\"password\""));
    }

    #[test]
    fn no_stored_password_is_reported_as_no_stored_password() {
        let mut row = row();
        row.password_sealed = None;

        assert!(!row.to_settings().has_password);
    }

    #[test]
    fn the_view_carries_every_field_a_screen_has_to_render() {
        let settings = row().to_settings();

        assert_eq!(settings.host, "smtp.example.com");
        assert_eq!(settings.port, 587);
        assert_eq!(settings.username, "postmaster");
        assert_eq!(settings.from_address, "no-reply@example.com");
        assert_eq!(settings.encryption, MailEncryption::StartTls);
    }
}
