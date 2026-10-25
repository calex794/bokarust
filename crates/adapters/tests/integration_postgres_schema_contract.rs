//! BKR-41 additive PostgreSQL contract checks.
//! Uses a disposable database: TEST_DATABASE_URL must never point at production.
use bokarust_adapters::postgres_accounts::PgAccounts;

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database via TEST_DATABASE_URL"]
async fn integration_bkr41_schema_constraints() {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must reference a disposable PostgreSQL");
    let accounts = PgAccounts::connect(&url).await.expect("connect");
    accounts.migrate().await.expect("apply every migration");
    let pool = accounts.pool();
    let mut tx = pool.begin().await.expect("transaction");

    // Tests are isolated by a rollback, including all foreign-key fixtures.
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash) \
         VALUES('bkr41-owner@example.invalid','Owner','$argon2id$test') RETURNING id",
    )
    .fetch_one(&mut *tx).await.expect("owner");
    let reader: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash) \
         VALUES('bkr41-reader@example.invalid','Reader','$argon2id$test') RETURNING id",
    )
    .fetch_one(&mut *tx).await.expect("reader");
    let book_a: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,owner_user_id) \
         VALUES('bkr41-source-a','ru','A',$1) RETURNING id",
    ).bind(owner).fetch_one(&mut *tx).await.expect("book A");
    let book_b: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,owner_user_id) \
         VALUES('bkr41-source-b','ru','B',$1) RETURNING id",
    ).bind(owner).fetch_one(&mut *tx).await.expect("book B");
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level) \
         VALUES($1,1,'Chapter','public','free') RETURNING id",
    ).bind(book_a).fetch_one(&mut *tx).await.expect("chapter");
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) \
         VALUES($1,1,'Body') RETURNING id",
    ).bind(chapter).fetch_one(&mut *tx).await.expect("version");

    // Valid version-anchored place requires no entitlement row (no grant made).
    sqlx::query(
        "INSERT INTO reader_version_positions \
         (user_id,chapter_id,version_id,block_index,char_offset) VALUES($1,$2,$3,1,0)",
    ).bind(reader).bind(chapter).bind(version)
        .execute(&mut *tx).await.expect("position");

    // A composite FK must prevent cross-book progress, independently of Rust.
    // Use a savepoint so a deliberately failed constraint does not abort the test.
    sqlx::query("SAVEPOINT bkr41_cross_book").execute(&mut *tx).await.expect("savepoint");
    let wrong = sqlx::query(
        "INSERT INTO reader_book_positions \
         (user_id,book_id,chapter_id,chapter_no,chapter_revision,version_id,block_index) \
         VALUES($1,$2,$3,1,0,$4,1)",
    ).bind(reader).bind(book_b).bind(chapter).bind(version)
        .execute(&mut *tx).await;
    assert!(wrong.is_err(), "cross-book progress must be rejected");
    sqlx::query("ROLLBACK TO SAVEPOINT bkr41_cross_book").execute(&mut *tx)
        .await.expect("rollback failed statement");

    let first_team: i64 = sqlx::query_scalar(
        "INSERT INTO teams(owner_user_id,name) VALUES($1,'T1') RETURNING id",
    ).bind(owner).fetch_one(&mut *tx).await.expect("team 1");
    let second_team: i64 = sqlx::query_scalar(
        "INSERT INTO teams(owner_user_id,name) VALUES($1,'T2') RETURNING id",
    ).bind(owner).fetch_one(&mut *tx).await.expect("team 2");
    sqlx::query("INSERT INTO team_book_links(team_id,book_id) VALUES($1,$2)")
        .bind(first_team).bind(book_a).execute(&mut *tx)
        .await.expect("first active team");

    sqlx::query("SAVEPOINT bkr41_team_conflict").execute(&mut *tx).await.expect("savepoint");
    let second_link = sqlx::query(
        "INSERT INTO team_book_links(team_id,book_id) VALUES($1,$2)",
    ).bind(second_team).bind(book_a).execute(&mut *tx).await;
    assert!(second_link.is_err(), "second active team must be rejected");
    sqlx::query("ROLLBACK TO SAVEPOINT bkr41_team_conflict").execute(&mut *tx)
        .await.expect("rollback failed statement");

    let evt: i64 = sqlx::query_scalar(
        "INSERT INTO outbox_events(aggregate_type,aggregate_id,event_type,dedup_key) \
         VALUES('chapter',$1,'published','bkr41-event-0000001') RETURNING id",
    ).bind(chapter).fetch_one(&mut *tx).await.expect("outbox");
    sqlx::query("SAVEPOINT bkr41_immutable").execute(&mut *tx).await.expect("savepoint");
    let mutated = sqlx::query("UPDATE outbox_events SET event_type='other' WHERE id=$1")
        .bind(evt).execute(&mut *tx).await;
    assert!(mutated.is_err(), "outbox payload must stay immutable");
    sqlx::query("ROLLBACK TO SAVEPOINT bkr41_immutable").execute(&mut *tx)
        .await.expect("rollback failed statement");

    sqlx::query(
        "INSERT INTO durable_jobs(kind,dedup_key,status) \
         VALUES('test','bkr41-task','queued')",
    ).execute(&mut *tx).await.expect("queued job");
    sqlx::query("SAVEPOINT bkr41_job_state").execute(&mut *tx).await.expect("savepoint");
    let invalid_job = sqlx::query(
        "UPDATE durable_jobs SET status='running' WHERE kind='test' AND dedup_key='bkr41-task'",
    ).execute(&mut *tx).await;
    assert!(invalid_job.is_err(), "running job must have a lease");
    sqlx::query("ROLLBACK TO SAVEPOINT bkr41_job_state").execute(&mut *tx)
        .await.expect("rollback failed statement");
    tx.rollback().await.expect("clean up fixtures");
}
