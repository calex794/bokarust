//! Buyer wallet checkout. No HTTP funding path and no creator payout posting.
#![forbid(unsafe_code)]
use sqlx::{PgPool, Row};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutError {
    Invalid,
    Unavailable,
    Missing,
    Changed,
    AlreadyOwned,
    InsufficientFunds,
}

#[derive(Debug, Clone)]
pub struct CheckoutOffer {
    pub chapter_id: i64,
    pub book_title: String,
    pub chapter_title: String,
    pub version_id: i64,
    pub price_minor: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckoutReceipt {
    pub purchase_id: i64,
    pub chapter_id: i64,
    pub replayed: bool,
}

#[derive(Clone)]
pub struct PgChapterCheckout {
    pool: PgPool,
}

fn db(_: sqlx::Error) -> CheckoutError {
    CheckoutError::Unavailable
}

fn valid_key(key: &str) -> bool {
    key.len() == 43
        && key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

impl PgChapterCheckout {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn balance(&self, user_id: i64) -> Result<i64, CheckoutError> {
        if user_id <= 0 {
            return Err(CheckoutError::Invalid);
        }
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?
        .unwrap_or(0))
    }

    /// An offer has only public metadata and no paid content. A new checkout
    /// checks the publication and price *again*, while holding DB locks.
    pub async fn offer(
        &self,
        user_id: i64,
        chapter_id: i64,
    ) -> Result<Option<CheckoutOffer>, CheckoutError> {
        if user_id <= 0 || chapter_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT c.id AS chapter_id, b.title AS book_title, c.title AS chapter_title, \
                    p.version_id, c.price_amount \
             FROM chapters AS c \
             JOIN books AS b ON b.id = c.book_id \
             JOIN chapter_publications AS p ON p.chapter_id = c.id \
             JOIN chapter_versions AS v ON v.chapter_id = c.id AND v.id = p.version_id \
             WHERE c.id = $1 AND c.visibility = 'public' AND c.access_level = 'paid' \
               AND c.price_amount > 0 AND NOT c.admin_hold AND NOT c.is_deleted \
               AND (c.reader_released_at IS NULL OR c.reader_released_at <= now()) \
               AND b.visibility = 'public' AND NOT b.admin_hold AND NOT b.is_deleted \
               AND (b.owner_user_id IS NULL OR b.owner_user_id <> $2)",
        )
        .bind(chapter_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        row.map(|row| {
            Ok(CheckoutOffer {
                chapter_id: row.try_get("chapter_id").map_err(db)?,
                book_title: row.try_get("book_title").map_err(db)?,
                chapter_title: row.try_get("chapter_title").map_err(db)?,
                version_id: row.try_get("version_id").map_err(db)?,
                price_minor: row.try_get("price_amount").map_err(db)?,
            })
        })
        .transpose()
    }

    /// All mutations commit or roll back together. Replay after an ambiguous
    /// network result consults the committed command before live price checks.
    pub async fn purchase(
        &self,
        user_id: i64,
        chapter_id: i64,
        expected_version_id: i64,
        expected_price_minor: i64,
        command_key: &str,
    ) -> Result<CheckoutReceipt, CheckoutError> {
        if user_id <= 0
            || chapter_id <= 0
            || expected_version_id <= 0
            || expected_price_minor <= 0
            || !valid_key(command_key)
        {
            return Err(CheckoutError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(db)?;

        // Unique row serialization means a same-key request cannot race another
        // into a second debit. It also detects same-key/different-payload reuse.
        sqlx::query(
            "INSERT INTO chapter_purchase_commands \
               (user_id, command_key, chapter_id, expected_version_id, expected_price_minor) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (user_id, command_key) DO NOTHING",
        )
        .bind(user_id)
        .bind(command_key)
        .bind(chapter_id)
        .bind(expected_version_id)
        .bind(expected_price_minor)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        let command = sqlx::query(
            "SELECT chapter_id, expected_version_id, expected_price_minor, purchase_id \
             FROM chapter_purchase_commands WHERE user_id = $1 AND command_key = $2 \
             FOR UPDATE",
        )
        .bind(user_id)
        .bind(command_key)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        if command.try_get::<i64, _>("chapter_id").map_err(db)? != chapter_id
            || command.try_get::<i64, _>("expected_version_id").map_err(db)? != expected_version_id
            || command.try_get::<i64, _>("expected_price_minor").map_err(db)? != expected_price_minor
        {
            return Err(CheckoutError::Changed);
        }
        if let Some(purchase_id) = command
            .try_get::<Option<i64>, _>("purchase_id")
            .map_err(db)?
        {
            tx.commit().await.map_err(db)?;
            return Ok(CheckoutReceipt {
                purchase_id,
                chapter_id,
                replayed: true,
            });
        }

        // The purchase pins precisely the version/price shown in the form.
        // Hold locks against publication, price and moderation mutations.
        let offer = sqlx::query(
            "SELECT p.version_id, c.price_amount \
             FROM chapters AS c \
             JOIN books AS b ON b.id = c.book_id \
             JOIN chapter_publications AS p ON p.chapter_id = c.id \
             JOIN chapter_versions AS v ON v.chapter_id = c.id AND v.id = p.version_id \
             WHERE c.id = $1 AND c.visibility = 'public' AND c.access_level = 'paid' \
               AND c.price_amount > 0 AND NOT c.admin_hold AND NOT c.is_deleted \
               AND (c.reader_released_at IS NULL OR c.reader_released_at <= now()) \
               AND b.visibility = 'public' AND NOT b.admin_hold AND NOT b.is_deleted \
               AND (b.owner_user_id IS NULL OR b.owner_user_id <> $2) \
             FOR SHARE OF c, p, b",
        )
        .bind(chapter_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?
        .ok_or(CheckoutError::Missing)?;
        if offer.try_get::<i64, _>("version_id").map_err(db)? != expected_version_id
            || offer.try_get::<i64, _>("price_amount").map_err(db)? != expected_price_minor
        {
            return Err(CheckoutError::Changed);
        }

        // One wallet row serializes all concurrent purchases by this user,
        // even with different command keys or across several server replicas.
        let wallet = sqlx::query_scalar::<_, i64>(
            "SELECT balance_minor FROM reader_wallet_accounts \
             WHERE user_id = $1 AND currency = 'RUB' FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        // Under the wallet lock, an already-owned chapter is never purchased
        // again under a fresh key, regardless of the amount of money available.
        let owned = sqlx::query_scalar::<_, i64>(
            "SELECT purchase_id FROM library_purchased_chapters \
             WHERE user_id = $1 AND chapter_id = $2",
        )
        .bind(user_id)
        .bind(chapter_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        if owned.is_some() {
            return Err(CheckoutError::AlreadyOwned);
        }
        let balance = wallet.unwrap_or(0);
        if balance < expected_price_minor {
            return Err(CheckoutError::InsufficientFunds);
        }
        let balance_after = balance - expected_price_minor;
        sqlx::query(
            "UPDATE reader_wallet_accounts SET balance_minor = $2 WHERE user_id = $1",
        )
        .bind(user_id)
        .bind(balance_after)
        .execute(&mut *tx)
        .await
        .map_err(db)?;

        let settlement = format!("wallet:{user_id}:{command_key}");
        // Existing purchase trigger creates the pinned LibraryItem in this
        // transaction. Any duplicate or foreign-key failure rolls back debit.
        let purchase_id: i64 = sqlx::query_scalar(
            "INSERT INTO chapter_purchase_facts \
             (user_id, chapter_id, version_id, settlement_reference, amount_minor, currency) \
             VALUES ($1, $2, $3, $4, $5, 'RUB') RETURNING id",
        )
        .bind(user_id)
        .bind(chapter_id)
        .bind(expected_version_id)
        .bind(&settlement)
        .bind(expected_price_minor)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;

        sqlx::query(
            "INSERT INTO reader_wallet_entries \
              (user_id, kind, delta_minor, balance_after_minor, reference, purchase_id) \
             VALUES ($1, 'chapter_purchase', $2, $3, $4, $5)",
        )
        .bind(user_id)
        .bind(-expected_price_minor)
        .bind(balance_after)
        .bind(&settlement)
        .bind(purchase_id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        sqlx::query(
            "UPDATE chapter_purchase_commands SET purchase_id = $3 \
             WHERE user_id = $1 AND command_key = $2",
        )
        .bind(user_id)
        .bind(command_key)
        .bind(purchase_id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(CheckoutReceipt {
            purchase_id,
            chapter_id,
            replayed: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_keys_accept_only_the_existing_43_character_token_format() {
        assert!(valid_key(&"x".repeat(43)));
        assert!(!valid_key("short"));
        assert!(!valid_key(&"x".repeat(44)));
        assert!(!valid_key(&format!("{}!", "x".repeat(42))));
    }
}
