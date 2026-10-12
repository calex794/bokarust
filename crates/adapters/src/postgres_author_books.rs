//! One transaction for creating a book and recording its retry result.
use crate::postgres_author::{claim, db, payload_digest, PgAuthor};
use bokarust_application::authoring::{AuthorError, NewBook};
use sqlx::Row;

impl PgAuthor {
    pub async fn create_book(
        &self,
        user_id: i64,
        input: &NewBook,
    ) -> Result<i64, AuthorError> {
        if user_id <= 0 {
            return Err(AuthorError::Forbidden);
        }
        let digest = payload_digest(&[
            b"new-book",
            input.source_identity.as_bytes(),
            input.target_language.as_bytes(),
            input.title.as_bytes(),
        ]);
        let mut tx = self.pool.begin().await.map_err(db)?;
        let fresh = claim(
            &mut tx,
            "INSERT INTO book_create_commands (user_id, command_key, payload_hash) \
             VALUES ($1, $2, $3) ON CONFLICT DO NOTHING RETURNING user_id",
            user_id,
            &input.command_key,
            &digest,
        )
        .await?;
        if !fresh {
            let row = sqlx::query(
                "SELECT payload_hash, book_id FROM book_create_commands \
                 WHERE user_id = $1 AND command_key = $2",
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
            return row.try_get::<Option<i64>, _>("book_id")
                .map_err(db)?
                .ok_or(AuthorError::Unavailable);
        }
        let book_id: i64 = sqlx::query_scalar(
            "INSERT INTO books (source_identity, target_language, title, visibility, owner_user_id) \
             VALUES ($1, $2, $3, 'public', $4) RETURNING id",
        )
        .bind(&input.source_identity)
        .bind(&input.target_language)
        .bind(&input.title)
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        sqlx::query(
            "UPDATE book_create_commands SET book_id = $3 \
             WHERE user_id = $1 AND command_key = $2",
        )
        .bind(user_id)
        .bind(&input.command_key)
        .bind(book_id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(book_id)
    }
}