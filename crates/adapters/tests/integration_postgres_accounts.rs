//! Run explicitly with TEST_DATABASE_URL and a disposable PostgreSQL database.

use bokarust_adapters::{
    account_security::{
        generate_session_token, hash_password, session_token_digest,
        verify_password,
    },
    postgres_accounts::PgAccounts,
};
use bokarust_domain::accounts::normalize_email;
use std::num::NonZeroU32;

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database via TEST_DATABASE_URL"]
async fn integration_postgres_users_and_revocable_sessions() {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to a disposable PostgreSQL database");
    let accounts = PgAccounts::connect(&url).await.expect("connect postgres");
    accounts.migrate().await.expect("apply migrations");

    let secret = generate_session_token();
    let email = normalize_email(&format!("  CI-{secret}@Example.INVALID "))
        .expect("normalize unique email");
    let password = "Пароль для теста с высокой энтропией!";
    let password_hash = hash_password(password).expect("argon2id");
    assert!(!password_hash.contains(password));

    let user_id = accounts
        .insert_user(&email, "Проверка", &password_hash)
        .await
        .expect("create user");
    let existing = accounts
        .find_user(&email)
        .await
        .expect("find user")
        .expect("user exists");
    assert_eq!(existing.id, user_id);
    assert_eq!(existing.email_normalized, email);
    assert_eq!(existing.display_name, "Проверка");
    assert!(verify_password(password, &existing.password_hash));
    assert!(!verify_password("Wrong", &existing.password_hash));

    // PostgreSQL, rather than a read-before-write check, arbitrates races.
    let duplicate = accounts
        .insert_user(&email, "Другой", &password_hash)
        .await;
    assert!(duplicate.is_err(), "duplicate normalized email must be rejected");

    let digest = session_token_digest(&secret);
    let ttl = NonZeroU32::new(300).expect("nonzero");
    accounts
        .insert_session(user_id, &digest, ttl)
        .await
        .expect("store only token digest");
    assert_eq!(accounts.active_session_user(&digest).await.unwrap(), Some(user_id));
    assert_eq!(
        accounts
            .active_session_user(&session_token_digest("different"))
            .await
            .unwrap(),
        None
    );

    assert!(accounts.revoke_session(&digest).await.expect("revoke session"));
    assert!(!accounts.revoke_session(&digest).await.expect("idempotent revoke"));
    assert_eq!(accounts.active_session_user(&digest).await.unwrap(), None);

    let next_token = generate_session_token();
    let next_digest = session_token_digest(&next_token);
    accounts
        .insert_session(user_id, &next_digest, ttl)
        .await
        .expect("insert second session");
    sqlx::query(
        "UPDATE sessions SET created_at = now() - INTERVAL '1 day', \
         expires_at = now() - INTERVAL '1 second' WHERE token_hash = $1",
    )
    .bind(next_digest.as_slice())
    .execute(accounts.pool())
    .await
    .expect("expire session");
    assert_eq!(accounts.active_session_user(&next_digest).await.unwrap(), None);

    // A test-only cleanup, not a production deletion API.
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(accounts.pool())
        .await
        .expect("cleanup test user");
}
