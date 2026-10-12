//! One SaveChapter transaction: authorize, compare revision, store text, publish.
use crate::postgres_author::{claim, db, payload_digest, PgAuthor};
use bokarust_application::authoring::{AuthorError, SaveChapter, SavedChapter};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};

async fn text_version(
    tx: &mut Transaction<'_, Postgres>,
    chapter_id: i64,
    body: &str,
) -> Result<i64, AuthorError> {
    let hash = Sha256::digest(body.as_bytes());
    let previous: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM chapter_versions WHERE chapter_id = $1 AND content_sha256 = $2",
    )
    .bind(chapter_id)
    .bind(hash.as_slice())
    .fetch_optional(&mut **tx)
    .await
    .map_err(db)?;
    if let Some(version_id) = previous {
        return Ok(version_id);
    }
    let next: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version_no), 0) + 1 FROM chapter_versions WHERE chapter_id = $1",
    )
    .bind(chapter_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(db)?;
    sqlx::query_scalar(
        "INSERT INTO chapter_versions (chapter_id, version_no, body_markdown, content_sha256) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(chapter_id)
    .bind(next)
    .bind(body)
    .bind(hash.as_slice())
    .fetch_one(&mut **tx)
    .await
    .map_err(db)
}

async fn existing_chapter(
    tx: &mut Transaction<'_, Postgres>,
    input: &SaveChapter,
) -> Result<(i64, i64, i64), AuthorError> {
    let chapter_id = input.chapter_id.ok_or(AuthorError::Invalid)?;
    let row = sqlx::query(
        "SELECT c.chapter_no, c.title, c.access_level, c.price_cents, c.edit_revision, \
                c.visibility, p.version_id, v.body_markdown \
         FROM chapters c \
         JOIN chapter_publications p ON p.chapter_id = c.id \
         JOIN chapter_versions v ON v.id = p.version_id AND v.chapter_id = c.id \
         WHERE c.id = $1 AND c.book_id = $2 AND NOT c.is_deleted AND NOT c.admin_hold \
         FOR UPDATE OF c",
    )
    .bind(chapter_id)
    .bind(input.book_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db)?
    .ok_or(AuthorError::Missing)?;
    let revision: i64 = row.try_get("edit_revision").map_err(db)?;
    let number: i32 = row.try_get("chapter_no").map_err(db)?;
    if revision != input.expected_revision || number != input.number {
        return Err(AuthorError::Conflict);
    }
    let published_version: i64 = row.try_get("version_id").map_err(db)?;
    let old_text: String = row.try_get("body_markdown").map_err(db)?;
    let unchanged = old_text == input.body
        && row.try_get::<String, _>("title").map_err(db)? == input.title
        && row.try_get::<String, _>("access_level").map_err(db)? == input.access.as_str()
        && row.try_get::<i64, _>("price_cents").map_err(db)? == input.price_cents
        && row.try_get::<String, _>("visibility").map_err(db)? == "public";
    if unchanged {
        return Ok((chapter_id, published_version, revision));
    }
    let version_id = if old_text == input.body {
        published_version
    } else {
        text_version(tx, chapter_id, &input.body).await?
    };
    sqlx::query(
        "UPDATE chapters SET title = $2, access_level = $3, price_cents = $4, \
         visibility = 'public', edit_revision = edit_revision + 1 WHERE id = $1",
    )
    .bind(chapter_id)
    .bind(&input.title)
    .bind(input.access.as_str())
    .bind(input.price_cents)
    .execute(&mut **tx)
    .await
    .map_err(db)?;
    sqlx::query(
        "UPDATE chapter_publications SET version_id = $2 WHERE chapter_id = $1",
    )
    .bind(chapter_id)
    .bind(version_id)
    .execute(&mut **tx)
    .await
    .map_err(db)?;
    Ok((chapter_id, version_id, revision + 1))
}

async fn new_chapter(
    tx: &mut Transaction<'_, Postgres>,
    input: &SaveChapter,
) -> Result<(i64, i64, i64), AuthorError> {
    let chapter_id: i64 = sqlx::query_scalar(
        "INSERT INTO chapters \
         (book_id, chapter_no, title, visibility, access_level, price_cents, edit_revision) \
         VALUES ($1, $2, $3, 'public', $4, $5, 1) RETURNING id",
    )
    .bind(input.book_id)
    .bind(input.number)
    .bind(&input.title)
    .bind(input.access.as_str())
    .bind(input.price_cents)
    .fetch_one(&mut **tx)
    .await
    .map_err(db)?;
    let version_id = text_version(tx, chapter_id, &input.body).await?;
    sqlx::query(
        "INSERT INTO chapter_publications (chapter_id, version_id) VALUES ($1, $2)",
    )
    .bind(chapter_id)
    .bind(version_id)
    .execute(&mut **tx)
    .await
    .map_err(db)?;
    // First release is a durable fact, independent of future notifications.
    sqlx::query(
        "INSERT INTO chapter_first_release_events (chapter_id, version_id) VALUES ($1, $2)",
    )
    .bind(chapter_id)
    .bind(version_id)
    .execute(&mut **tx)
    .await
    .map_err(db)?;
    Ok((chapter_id, version_id, 1))
}

impl PgAuthor {
    pub async fn save_chapter(
        &self,
        user_id: i64,
        input: &SaveChapter,
    ) -> Result<SavedChapter, AuthorError> {
        if user_id <= 0 {
            return Err(AuthorError::Forbidden);
        }
        let requested_chapter = input.chapter_id.unwrap_or(0).to_be_bytes();
        let book_id = input.book_id.to_be_bytes();
        let number = input.number.to_be_bytes();
        let expected = input.expected_revision.to_be_bytes();
        let price = input.price_cents.to_be_bytes();
        let digest = payload_digest(&[
            b"save-chapter",
            &book_id,
            &requested_chapter,
            &number,
            &expected,
            input.title.as_bytes(),
            input.body.as_bytes(),
            input.access.as_str().as_bytes(),
            &price,
        ]);
        let mut tx = self.pool.begin().await.map_err(db)?;
        let fresh = claim(
            &mut tx,
            "INSERT INTO chapter_save_commands (user_id, command_key, payload_hash) \
             VALUES ($1, $2, $3) ON CONFLICT DO NOTHING RETURNING user_id",
            user_id,
            &input.command_key,
            &digest,
        )
        .await?;
        // The database checks ownership on EACH write, not only when the form loads.
        let allowed: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM books WHERE id = $1 AND owner_user_id = $2 \
             AND NOT admin_hold AND NOT is_deleted AND visibility = 'public' FOR SHARE",
        )
        .bind(input.book_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        if allowed.is_none() {
            return Err(AuthorError::Forbidden);
        }
        if !fresh {
            let row = sqlx::query(
                "SELECT payload_hash, chapter_id, version_id, saved_revision \
                 FROM chapter_save_commands WHERE user_id = $1 AND command_key = $2",
            )
            .bind(user_id)
            .bind(&input.command_key)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
            let previous: Vec<u8> = row.try_get("payload_hash").map_err(db)?;
            if previous != digest {
                return Err(AuthorError::Conflict);
            }
            return Ok(SavedChapter {
                chapter_id: row.try_get::<Option<i64>, _>("chapter_id").map_err(db)?
                    .ok_or(AuthorError::Unavailable)?,
                version_id: row.try_get::<Option<i64>, _>("version_id").map_err(db)?
                    .ok_or(AuthorError::Unavailable)?,
                revision: row.try_get::<Option<i64>, _>("saved_revision").map_err(db)?
                    .ok_or(AuthorError::Unavailable)?,
            });
        }
        let (chapter_id, version_id, revision) = if input.chapter_id.is_some() {
            existing_chapter(&mut tx, input).await?
        } else {
            new_chapter(&mut tx, input).await?
        };
        sqlx::query(
            "UPDATE chapter_save_commands \
             SET chapter_id = $3, version_id = $4, saved_revision = $5 \
             WHERE user_id = $1 AND command_key = $2",
        )
        .bind(user_id)
        .bind(&input.command_key)
        .bind(chapter_id)
        .bind(version_id)
        .bind(revision)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(SavedChapter {
            chapter_id,
            version_id,
            revision,
        })
    }
}