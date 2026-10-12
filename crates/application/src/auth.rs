//! Input policy for account creation, independent of Axum and PostgreSQL.
use bokarust_domain::accounts::normalize_email;

pub struct NewAccount {
    pub email_normalized: String,
    pub display_name: String,
}

pub fn validate_registration(
    email: &str,
    name: &str,
    password: &str,
) -> Result<NewAccount, &'static str> {
    let email_normalized = normalize_email(email)?;
    let display_name = name.trim();
    if display_name.is_empty()
        || display_name.chars().count() > 80
        || display_name.chars().any(char::is_control)
    {
        return Err("Имя должно содержать от 1 до 80 символов");
    }
    let length = password.chars().count();
    if !(12..=128).contains(&length) || password.len() > 1024 || password.trim().is_empty() {
        return Err("Пароль должен содержать от 12 до 128 символов");
    }
    Ok(NewAccount {
        email_normalized,
        display_name: display_name.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_unicode_names_and_password_length() {
        let value = validate_registration(
            "  ALICE@Example.com ",
            "  Алиса  ",
            "достаточно длинный пароль",
        )
        .unwrap();
        assert_eq!(value.email_normalized, "alice@example.com");
        assert_eq!(value.display_name, "Алиса");
        assert!(validate_registration("a@b.com", "x", "short").is_err());
        assert!(validate_registration("a@b.com", "x", &"a".repeat(129)).is_err());
        assert!(validate_registration("a@b.com", "<script>", "long-enough-pass").is_ok());
        assert!(validate_registration("a@b.com", "a\nb", "long-enough-pass").is_err());
    }
}
