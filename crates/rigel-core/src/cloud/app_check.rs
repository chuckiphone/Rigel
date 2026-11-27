//! Roadmap 165: App Check enforcement for projects that configure it.
//!
//! Attestation lives here rather than scattered through the HTTP methods. A project with no
//! [`AppCheckConfig`] passes through unchanged; a project that configures one must produce a
//! token before any Firebase request is sent. An un-attested client gets a named error from
//! `docs/troubleshooting.md`, never a silent unauthenticated request that Firebase would reject.

use serde::Deserialize;
use tokio::sync::Mutex as AsyncMutex;

use super::{CloudError, now_ms, rfc3339, truncate};

const APP_CHECK_MARGIN_MS: i64 = 60_000;

/// The client half of a custom App Check provider.
///
/// It is deliberately not a Firebase SDK: Rigel asks the endpoint the user configured for an
/// App Check token the Firebase Admin SDK minted for this project's app, then attaches it to
/// every Firebase request. The token is a bearer credential, so it lives in memory only and is
/// never serialized, logged or sent to the renderer.
pub(crate) struct AppCheck {
    pub(crate) attest_url: String,
    /// The per-install credential the endpoint checks before minting a token. `None` means this
    /// installation has not been enrolled; the endpoint decides whether that is acceptable.
    client_token: Option<String>,
    http: reqwest::Client,
    cached: AsyncMutex<Option<CachedAppCheckToken>>,
}

struct CachedAppCheckToken {
    token: String,
    expires_at: i64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AppCheckAnswer {
    pub(crate) token: String,
    /// Unix seconds. Milliseconds are refused by the caller, not silently reinterpreted.
    pub(crate) expires_at: i64,
}

/// The sentence a 401/403 from the attestation endpoint produces. The endpoint is reachable, so it
/// must never read like the offline error; the two fixes are different (enrol this install versus
/// start the endpoint).
pub(crate) fn app_check_refusal(status: reqwest::StatusCode, has_client_token: bool) -> String {
    if has_client_token {
        format!(
            "the attestation endpoint refused this client ({status}); the stored client token is not on its allowlist"
        )
    } else {
        format!(
            "the attestation endpoint refused this client ({status}); set the App Check client token for this installation"
        )
    }
}

/// The error an attested client must not receive: a configured project never sends a request
/// without a token. The sentence is copied verbatim into `docs/troubleshooting.md`.
pub const EMPTY_TOKEN_ERROR: &str = "the attestation endpoint returned an empty token";

/// Enforce the gate for one request. `None` means App Check is off and the request passes through;
/// `Some(check)` means a token is mandatory and a failure aborts the request.
pub(crate) async fn enforced_token(check: Option<&AppCheck>) -> Result<Option<String>, CloudError> {
    match check {
        Some(check) => {
            let token = check.token().await?;
            require_attestation(true, Some(&token))?;
            Ok(Some(token))
        }
        None => Ok(None),
    }
}

/// Check a supplied token against the project's configuration, without any network work. This is
/// the rule the HTTP gate applies and the same rule the CLI and tests use to prove it.
pub fn require_attestation(
    configured: bool,
    token: Option<&str>,
) -> Result<Option<String>, CloudError> {
    if !configured {
        return Ok(None);
    }
    match token.map(str::trim).filter(|token| !token.is_empty()) {
        Some(token) => Ok(Some(token.to_string())),
        None => Err(CloudError::AppCheck(EMPTY_TOKEN_ERROR.to_string())),
    }
}

impl AppCheck {
    pub(crate) fn new(
        attest_url: String,
        client_token: Option<String>,
        http: reqwest::Client,
    ) -> Self {
        Self {
            attest_url,
            client_token: client_token.filter(|token| !token.trim().is_empty()),
            http,
            cached: AsyncMutex::new(None),
        }
    }

    pub(crate) fn has_client_token(&self) -> bool {
        self.client_token.is_some()
    }

    /// A token good for at least another minute, fetching one when the cache is empty or close
    /// to expiry. The lock spans the fetch on purpose: a burst of Firestore commits should cost
    /// one attestation, not one per request.
    pub(crate) async fn token(&self) -> Result<String, CloudError> {
        let mut cached = self.cached.lock().await;
        if let Some(current) = cached.as_ref()
            && current.expires_at - now_ms() > APP_CHECK_MARGIN_MS
        {
            return Ok(current.token.clone());
        }

        let answer = self.fetch().await?;
        let token = require_attestation(true, Some(&answer.token))?
            .ok_or_else(|| CloudError::AppCheck(EMPTY_TOKEN_ERROR.to_string()))?;

        let now = now_ms();
        let expires_at = answer.expires_at.saturating_mul(1_000);
        if expires_at <= now {
            return Err(CloudError::AppCheck(format!(
                "the attestation endpoint returned a token that expired at {}",
                rfc3339(expires_at)
            )));
        }
        // Firebase's own ceiling for a custom provider is seven days. A larger number is almost
        // certainly milliseconds, which would otherwise be cached as if it never expired.
        if expires_at > now + 7 * 24 * 60 * 60 * 1_000 {
            return Err(CloudError::AppCheck(
                "`expires_at` is more than seven days away; it must be Unix seconds, not milliseconds"
                    .into(),
            ));
        }

        *cached = Some(CachedAppCheckToken {
            token: token.clone(),
            expires_at,
        });
        Ok(token)
    }

    async fn fetch(&self) -> Result<AppCheckAnswer, CloudError> {
        let mut request = self
            .http
            .get(&self.attest_url)
            .header("accept", "application/json");
        if let Some(token) = &self.client_token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(|error| {
            CloudError::AppCheck(format!(
                "the attestation endpoint is not reachable: {error}"
            ))
        })?;
        let status = response.status();
        let text = response.text().await.map_err(|error| {
            CloudError::AppCheck(format!(
                "the attestation endpoint's answer could not be read: {error}"
            ))
        })?;
        if !status.is_success() {
            let detail = text.trim();
            let message = if matches!(status.as_u16(), 401 | 403) {
                app_check_refusal(status, self.has_client_token())
            } else if detail.is_empty() {
                format!("the attestation endpoint answered {status}")
            } else {
                format!(
                    "the attestation endpoint answered {status}: {}",
                    truncate(detail, 200)
                )
            };
            return Err(CloudError::AppCheck(message));
        }
        serde_json::from_str(&text).map_err(|error| {
            CloudError::AppCheck(format!(
                "the attestation endpoint must answer with {{\"token\": \"â€¦\", \"expires_at\": <unix seconds>}}: {error}"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::AppCheckConfig;
    use super::*;

    #[test]
    fn configured_without_a_token_is_refused_with_the_documented_error() {
        let error =
            require_attestation(true, None).expect_err("a configured project needs a token");
        assert!(matches!(error, CloudError::AppCheck(_)));
        assert!(error.to_string().contains(EMPTY_TOKEN_ERROR), "{error}");
        let blank = require_attestation(true, Some("   ")).expect_err("whitespace is not a token");
        assert!(blank.to_string().contains(EMPTY_TOKEN_ERROR), "{blank}");
    }

    #[test]
    fn a_valid_token_is_attached_and_an_unconfigured_project_passes_through() {
        assert_eq!(
            require_attestation(true, Some(" attested-token ")).unwrap(),
            Some("attested-token".to_string())
        );
        assert_eq!(require_attestation(false, None).unwrap(), None);
        assert_eq!(require_attestation(false, Some("ignored")).unwrap(), None);
    }

    #[test]
    fn a_project_config_maps_to_the_same_gate() {
        // `AppCheckConfig` is the on/off switch the project stores; the gate never guesses a token.
        let config = Some(AppCheckConfig {
            attest_url: "http://127.0.0.1:9/attest".to_string(),
        });
        let configured = config.is_some();
        assert!(
            require_attestation(configured, None)
                .unwrap_err()
                .to_string()
                .contains(EMPTY_TOKEN_ERROR)
        );
        assert_eq!(
            require_attestation(configured, Some("t")).unwrap(),
            Some("t".to_string())
        );
    }

    #[test]
    fn every_refusal_is_named_in_the_troubleshooting_cookbook() {
        let book = include_str!("../../../../docs/troubleshooting.md");
        for message in [
            EMPTY_TOKEN_ERROR.to_string(),
            app_check_refusal(reqwest::StatusCode::FORBIDDEN, false),
            app_check_refusal(reqwest::StatusCode::UNAUTHORIZED, true),
            "the attestation endpoint is not reachable".to_string(),
            "the attestation endpoint must answer with".to_string(),
        ] {
            assert!(book.contains(&message), "missing cookbook entry: {message}");
        }
    }

    #[test]
    fn an_attestation_answer_needs_a_token_and_unix_seconds() {
        let answer: AppCheckAnswer =
            serde_json::from_str(r#"{"token":"t","expires_at":1760961600}"#).unwrap();
        assert_eq!(
            require_attestation(true, Some(&answer.token)).unwrap(),
            Some("t".to_string())
        );
        assert!(serde_json::from_str::<AppCheckAnswer>(r#"{"token":"t"}"#).is_err());
        assert!(
            serde_json::from_str::<AppCheckAnswer>(r#"{"token":"t","expires_at":"1760961600"}"#)
                .is_err(),
            "a string is not the number the contract asks for"
        );
    }

    #[tokio::test]
    async fn a_reachable_endpoint_that_returns_no_token_refuses_the_client() {
        // Bind a local listener so the refusal comes from a real HTTP answer, not a mock.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            // A failed token is not cached, so the gate fetches again; serve both requests.
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let mut buffer = [0u8; 1024];
                let _ = socket.read(&mut buffer).await;
                let body = r#"{"token":"  ","expires_at":4102444800}"#;
                let answer = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(answer.as_bytes()).await;
            }
        });
        let check = AppCheck::new(
            format!("http://{address}/attest"),
            None,
            reqwest::Client::new(),
        );
        let error = check
            .token()
            .await
            .expect_err("an empty token is not attestation");
        assert!(error.to_string().contains(EMPTY_TOKEN_ERROR), "{error}");
        assert!(
            enforced_token(Some(&check))
                .await
                .unwrap_err()
                .to_string()
                .contains(EMPTY_TOKEN_ERROR)
        );
        assert!(
            enforced_token(None).await.unwrap().is_none(),
            "off means pass-through"
        );
    }
}
