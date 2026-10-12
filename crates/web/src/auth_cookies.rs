//! Host-only cookies and stateless double-submit CSRF for native HTML forms.
use axum::{
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use bokarust_adapters::account_security::generate_session_token;
use subtle::ConstantTimeEq;

pub const SESSION_SECONDS: u32 = 7 * 24 * 60 * 60;

#[derive(Clone, Copy)]
pub struct CookiePolicy {
    pub secure: bool,
}

impl CookiePolicy {
    fn session_name(self) -> &'static str {
        if self.secure {
            "__Host-bokarust_sid"
        } else {
            "bokarust_dev_sid"
        }
    }

    fn csrf_name(self) -> &'static str {
        if self.secure {
            "__Host-bokarust_csrf"
        } else {
            "bokarust_dev_csrf"
        }
    }

    fn attributes(self) -> &'static str {
        if self.secure {
            "Path=/; Secure; HttpOnly; SameSite=Strict"
        } else {
            "Path=/; HttpOnly; SameSite=Strict"
        }
    }

    pub fn session(self, headers: &HeaderMap) -> Option<String> {
        cookie(headers, self.session_name())
    }

    pub fn csrf(self, headers: &HeaderMap) -> Option<String> {
        cookie(headers, self.csrf_name())
    }

    pub fn csrf_for_page(self, headers: &HeaderMap) -> (String, HeaderValue) {
        let secret = self.csrf(headers).unwrap_or_else(generate_session_token);
        let set_cookie = HeaderValue::from_str(&format!(
            "{}={secret}; {}; Max-Age=3600",
            self.csrf_name(),
            self.attributes()
        ))
        .expect("generated token is safe for HTTP headers");
        (secret, set_cookie)
    }

    pub fn verify_csrf(self, headers: &HeaderMap, supplied: &str) -> bool {
        let Some(cookie) = self.csrf(headers) else {
            return false;
        };
        valid_secret(supplied) && bool::from(cookie.as_bytes().ct_eq(supplied.as_bytes()))
    }

    pub fn set_session(self, token: &str) -> HeaderValue {
        assert!(valid_secret(token));
        HeaderValue::from_str(&format!(
            "{}={token}; {}; Max-Age={SESSION_SECONDS}",
            self.session_name(),
            self.attributes()
        ))
        .expect("generated token is safe for HTTP headers")
    }

    pub fn clear_session(self) -> HeaderValue {
        HeaderValue::from_str(&format!(
            "{}=; {}; Max-Age=0",
            self.session_name(),
            self.attributes()
        ))
        .expect("constant cookie header")
    }

    pub fn clear_csrf(self) -> HeaderValue {
        HeaderValue::from_str(&format!(
            "{}=; {}; Max-Age=0",
            self.csrf_name(),
            self.attributes()
        ))
        .expect("constant cookie header")
    }
}

fn valid_secret(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn cookie(headers: &HeaderMap, wanted: &str) -> Option<String> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE).iter() {
        let value = value.to_str().ok()?;
        for part in value.split(';') {
            let Some((name, value)) = part.trim().split_once('=') else {
                continue;
            };
            if name.trim() == wanted {
                if found.is_some() || !valid_secret(value) {
                    return None;
                }
                found = Some(value.to_owned());
            }
        }
    }
    found
}

/// Disable storage and embedding for authenticated responses.
pub fn protect(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'none'; style-src 'self'; form-action 'self'; \
             base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    response
}

pub fn redirect(path: &'static str) -> Response {
    protect(Redirect::to(path).into_response())
}

pub fn error(status: StatusCode) -> Response {
    protect(status.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_cookies_are_strict_and_duplicate_csrf_is_rejected() {
        let policy = CookiePolicy { secure: true };
        let headers = HeaderMap::new();
        let (token, set_cookie) = policy.csrf_for_page(&headers);
        let set = set_cookie.to_str().unwrap();
        assert!(set.starts_with("__Host-bokarust_csrf="));
        assert!(set.contains("Secure; HttpOnly; SameSite=Strict"));
        let mut with_cookie = HeaderMap::new();
        with_cookie.insert(
            header::COOKIE,
            format!("__Host-bokarust_csrf={token}").parse().unwrap(),
        );
        assert!(policy.verify_csrf(&with_cookie, &token));
        assert!(!policy.verify_csrf(&with_cookie, &"x".repeat(43)));
        with_cookie.append(
            header::COOKIE,
            format!("__Host-bokarust_csrf={token}").parse().unwrap(),
        );
        assert!(!policy.verify_csrf(&with_cookie, &token));
    }

    #[test]
    fn dev_cookie_never_misuses_host_prefix() {
        let policy = CookiePolicy { secure: false };
        let (token, _) = policy.csrf_for_page(&HeaderMap::new());
        assert!(policy
            .set_session(&token)
            .to_str()
            .unwrap()
            .starts_with("bokarust_dev_sid="));
    }
}
