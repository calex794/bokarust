//! Browser-side account security: cookies, CSRF and safe responses.
#![forbid(unsafe_code)]

use axum::{
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use bokarust_adapters::account_security::generate_session_token;

pub(crate) const SESSION_AGE: u32 = 7 * 24 * 60 * 60;
const CSRF_AGE: u32 = 60 * 60;

#[derive(Clone)]
pub(crate) struct CookiePolicy {
    pub secure: bool,
    pub origin: String,
}

impl CookiePolicy {
    fn session_name(&self) -> &'static str {
        if self.secure {
            "__Host-bkr-session"
        } else {
            "bkr-dev-session"
        }
    }

    fn csrf_name(&self) -> &'static str {
        if self.secure {
            "__Host-bkr-csrf"
        } else {
            "bkr-dev-csrf"
        }
    }

    fn cookie(&self, name: &str, value: &str, age: u32) -> String {
        let secure = if self.secure { "; Secure" } else { "" };
        format!(
            "{name}={value}; Path=/; Max-Age={age}; HttpOnly; SameSite=Strict{secure}"
        )
    }

    pub(crate) fn csrf_token_cookie(&self) -> (String, String) {
        let token = generate_session_token();
        let cookie = self.cookie(self.csrf_name(), &token, CSRF_AGE);
        (token, cookie)
    }

    pub(crate) fn session_cookie(&self, token: &str) -> String {
        self.cookie(self.session_name(), token, SESSION_AGE)
    }

    pub(crate) fn cleared_cookies(&self) -> [String; 2] {
        [
            self.cookie(self.session_name(), "", 0),
            self.cookie(self.csrf_name(), "", 0),
        ]
    }

    pub(crate) fn session_token(&self, headers: &HeaderMap) -> Option<String> {
        let token = cookie_value(headers, self.session_name())?;
        if !valid_token(&token) {
            return None;
        }
        Some(token)
    }

    pub(crate) fn valid_csrf(&self, headers: &HeaderMap, posted: &str) -> bool {
        let Some(cookie) = cookie_value(headers, self.csrf_name()) else {
            return false;
        };
        valid_token(&cookie) && constant_time_equal(cookie.as_bytes(), posted.as_bytes())
    }

    pub(crate) fn same_origin(&self, headers: &HeaderMap) -> bool {
        if headers
            .get("sec-fetch-site")
            .and_then(|v| v.to_str().ok())
            == Some("cross-site")
        {
            return false;
        }
        match headers.get(header::ORIGIN) {
            Some(origin) => origin.to_str().ok() == Some(self.origin.as_str()),
            None => true, // CSRF token remains mandatory even without Origin.
        }
    }
}

/// Reject duplicate cookie names rather than choosing an attacker-selected one.
fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut result = None;
    for line in headers.get_all(header::COOKIE) {
        let value = line.to_str().ok()?;
        for field in value.split(';') {
            let Some((key, value)) = field.trim().split_once('=') else {
                continue;
            };
            if key.trim() == name {
                if result.is_some() {
                    return None;
                }
                result = Some(value.trim().to_owned());
            }
        }
    }
    result
}

fn valid_token(token: &str) -> bool {
    token.len() == 43
        && token
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == b'-' || ch == b'_')
}

fn constant_time_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0_u8, |diff, (left, right)| diff | (left ^ right)) == 0
}

pub(crate) fn private_response<T: IntoResponse>(
    result: T,
    cookies: &[String],
) -> Response {
    let mut response = result.into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    for cookie in cookies {
        if let Ok(mut value) = HeaderValue::from_str(cookie) {
            value.set_sensitive(true);
            response.headers_mut().append(header::SET_COOKIE, value);
        } else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(secure: bool) -> CookiePolicy {
        CookiePolicy {
            secure,
            origin: "https://books.example".into(),
        }
    }

    #[test]
    fn production_cookies_restrict_subdomains_and_scripts() {
        let config = policy(true);
        let (token, csrf) = config.csrf_token_cookie();
        let session = config.session_cookie(&token);
        for cookie in [csrf, session] {
            assert!(cookie.starts_with("__Host-"));
            assert!(cookie.contains("; Secure"));
            assert!(cookie.contains("; HttpOnly"));
            assert!(cookie.contains("; SameSite=Strict"));
            assert!(cookie.contains("; Path=/"));
            assert!(!cookie.contains("Domain="));
        }
        assert!(config.cleared_cookies().iter().all(|x| x.contains("Max-Age=0")));
        assert!(!policy(false).session_cookie(&token).contains("; Secure"));
    }

    #[test]
    fn csrf_requires_single_cookie_and_matching_hidden_token() {
        let config = policy(true);
        let (token, value) = config.csrf_token_cookie();
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, format!("{}={}", config.csrf_name(), token).parse().unwrap());
        assert!(config.valid_csrf(&headers, &token));
        assert!(!config.valid_csrf(&headers, "invalid"));
        headers.insert(
            header::COOKIE,
            format!("{}={}; {}={}", config.csrf_name(), token, config.csrf_name(), token)
                .parse()
                .unwrap(),
        );
        assert!(!config.valid_csrf(&headers, &token));
        assert!(value.contains("Max-Age=3600"));
    }

    #[test]
    fn cross_origin_is_rejected_even_with_a_correct_csrf_token() {
        let config = policy(true);
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        assert!(!config.same_origin(&headers));
        headers.insert(header::ORIGIN, "https://books.example".parse().unwrap());
        assert!(config.same_origin(&headers));
        headers.insert("sec-fetch-site", "cross-site".parse().unwrap());
        assert!(!config.same_origin(&headers));
    }
}
