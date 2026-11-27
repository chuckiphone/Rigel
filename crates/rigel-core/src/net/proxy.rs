//! Shared system and explicit HTTP proxy configuration.

use std::sync::{OnceLock, RwLock};

use reqwest::{ClientBuilder, NoProxy, Proxy, Url};
use thiserror::Error;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxySettings {
    /// Optional explicit proxy URL, for example `http://proxy.example:8080`.
    pub explicit: Option<String>,
    /// Hosts or domains that should connect directly, in reqwest's no-proxy syntax.
    pub bypass: Vec<String>,
}

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("invalid proxy URL: {0}")]
    InvalidUrl(String),
}

fn settings_cell() -> &'static RwLock<ProxySettings> {
    static SETTINGS: OnceLock<RwLock<ProxySettings>> = OnceLock::new();
    SETTINGS.get_or_init(|| {
        let explicit = std::env::var("RIGEL_HTTP_PROXY")
            .ok()
            .filter(|value| !value.is_empty());
        let bypass = std::env::var("RIGEL_NO_PROXY")
            .or_else(|_| std::env::var("NO_PROXY"))
            .or_else(|_| std::env::var("no_proxy"))
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|host| !host.is_empty())
            .map(str::to_owned)
            .collect();
        RwLock::new(ProxySettings { explicit, bypass })
    })
}

/// Replace process-wide proxy settings used by subsequently built clients.
pub fn set_settings(settings: ProxySettings) -> Result<(), ProxyError> {
    if let Some(proxy) = settings.explicit.as_deref() {
        Url::parse(proxy).map_err(|_| ProxyError::InvalidUrl(proxy.to_owned()))?;
    }
    *settings_cell()
        .write()
        .unwrap_or_else(|poison| poison.into_inner()) = settings;
    Ok(())
}

/// Return the current explicit override and bypass list.
pub fn settings() -> ProxySettings {
    settings_cell()
        .read()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
}

/// Apply the common proxy policy to a reqwest builder.
///
/// Reqwest's system-proxy support remains enabled when no explicit override is configured.
/// An explicit override takes precedence over the system proxy, and its bypass list connects
/// matching hosts directly.
pub fn configure_builder(builder: ClientBuilder) -> ClientBuilder {
    configure_builder_with_settings(builder, &settings())
}

/// Apply an explicit settings value, primarily useful to consumers that scope configuration.
pub fn configure_builder_with_settings(
    mut builder: ClientBuilder,
    settings: &ProxySettings,
) -> ClientBuilder {
    if let Some(explicit) = settings.explicit.as_deref()
        && let Ok(mut proxy) = Proxy::all(explicit)
    {
        // With an explicit override, disable reqwest's automatic system proxy so a bypassed
        // destination cannot fall through to that proxy after the override declines it.
        builder = builder.no_proxy();
        let bypass = settings.bypass.join(",");
        if !bypass.is_empty() {
            proxy = proxy.no_proxy(NoProxy::from_string(&bypass));
        }
        builder = builder.proxy(proxy);
    }
    builder
}

/// Testable host matching used by the bypass list and MCP endpoint routing.
pub fn bypasses(settings: &ProxySettings, url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    settings.bypass.iter().any(|entry| {
        let entry = entry.trim().trim_start_matches('.');
        !entry.is_empty()
            && (host.eq_ignore_ascii_case(entry)
                || host
                    .to_ascii_lowercase()
                    .strip_suffix(&format!(".{entry}"))
                    .is_some())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn explicit_proxy_and_system_default_are_resolved() {
        set_settings(ProxySettings {
            explicit: Some("http://proxy.test:8080".into()),
            bypass: vec![],
        })
        .unwrap();
        assert_eq!(
            settings().explicit.as_deref(),
            Some("http://proxy.test:8080")
        );

        set_settings(ProxySettings::default()).unwrap();
        let client = configure_builder(ClientBuilder::new()).build();
        assert!(client.is_ok(), "system proxy configuration must build");
    }

    #[test]
    fn bypass_matches_exact_hosts_and_subdomains_only() {
        let settings = ProxySettings {
            explicit: Some("http://proxy.test:8080".into()),
            bypass: vec!["example.test".into(), "localhost".into()],
        };
        assert!(bypasses(
            &settings,
            &Url::parse("https://example.test/path").unwrap()
        ));
        assert!(bypasses(
            &settings,
            &Url::parse("https://api.example.test/path").unwrap()
        ));
        assert!(!bypasses(
            &settings,
            &Url::parse("https://notexample.test/path").unwrap()
        ));
    }

    #[tokio::test]
    async fn explicit_proxy_bypass_connects_directly() {
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = target.local_addr().unwrap();
        let serve_target = tokio::spawn(async move {
            let (mut stream, _) = target.accept().await.unwrap();
            let mut request = [0; 512];
            let count = stream.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..count]).starts_with("GET /health "));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
        });
        let settings = ProxySettings {
            explicit: Some("http://127.0.0.1:1".into()),
            bypass: vec!["127.0.0.1".into()],
        };
        let client = configure_builder_with_settings(ClientBuilder::new(), &settings)
            .build()
            .unwrap();
        let response = client
            .get(format!("http://{address}/health"))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        serve_target.await.unwrap();
    }
}
