//! Linear issue access with OAuth credentials isolated by project.
//!
//! Tokens are stored in the OS credential store under a key derived from the canonical project
//! root. They are never written to the project database or included in errors.

use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::credentials::{self, CredentialStore, KeyError, KeychainCredentialStore};

const LINEAR_ENDPOINT: &str = "https://api.linear.app/graphql";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LinearIssue {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub url: String,
    pub status: Option<String>,
}

/// A UI caller can render this action instead of retrying a revoked credential.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReconnectAction {
    pub action: String,
    pub integration: String,
    pub project_key: String,
    pub label: String,
}

#[derive(Debug, thiserror::Error)]
pub enum LinearError {
    #[error("no Linear OAuth token is configured for this project")]
    MissingToken,
    #[error("Linear authorization was revoked")]
    ReconnectRequired(ReconnectAction),
    #[error("Linear request failed: {0}")]
    Request(String),
    #[error("Linear returned an invalid response: {0}")]
    InvalidResponse(String),
    #[error("could not access the credential store: {0}")]
    Credential(String),
    #[error("could not configure Linear HTTP client: {0}")]
    HttpClient(#[from] crate::net::client::ClientError),
}

impl LinearError {
    pub fn reconnect_action(&self) -> Option<&ReconnectAction> {
        match self {
            Self::ReconnectRequired(action) => Some(action),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct LinearResponse {
    pub status: u16,
    pub body: String,
}

#[async_trait]
pub trait LinearTransport: Send + Sync {
    async fn post(
        &self,
        endpoint: &str,
        token: &str,
        body: Value,
    ) -> Result<LinearResponse, String>;
}

pub struct HttpLinearTransport {
    client: Arc<crate::net::client::ProviderClient>,
}

impl HttpLinearTransport {
    pub fn new() -> Result<Self, LinearError> {
        Ok(Self {
            client: crate::net::client::shared_provider_client("linear")?,
        })
    }
}

#[async_trait]
impl LinearTransport for HttpLinearTransport {
    async fn post(
        &self,
        endpoint: &str,
        token: &str,
        body: Value,
    ) -> Result<LinearResponse, String> {
        let response = self
            .client
            .run_phase(crate::net::client::HttpPhase::Response, async {
                self.client
                    .client()
                    .post(endpoint)
                    .bearer_auth(token)
                    .json(&body)
                    .send()
                    .await
            })
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        let status = response.status().as_u16();
        let body = self
            .client
            .run_phase(crate::net::client::HttpPhase::Body, response.text())
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        Ok(LinearResponse { status, body })
    }
}

pub struct LinearConnector {
    credentials: Arc<dyn CredentialStore>,
    transport: Arc<dyn LinearTransport>,
    endpoint: String,
}

impl LinearConnector {
    pub fn new(credentials: Arc<dyn CredentialStore>, transport: Arc<dyn LinearTransport>) -> Self {
        Self {
            credentials,
            transport,
            endpoint: LINEAR_ENDPOINT.to_string(),
        }
    }

    pub fn with_default_transport() -> Result<Self, LinearError> {
        Ok(Self::new(
            Arc::new(KeychainCredentialStore),
            Arc::new(HttpLinearTransport::new()?),
        ))
    }

    /// Store an OAuth token in the OS credential store for this project only.
    pub fn set_project_token(project: &Path, token: &str) -> Result<(), LinearError> {
        credentials::set(&credential_name(project), token)
            .map_err(|error| LinearError::Credential(error.to_string()))
    }

    pub fn clear_project_token(project: &Path) -> Result<(), LinearError> {
        credentials::delete(&credential_name(project))
            .map_err(|error| LinearError::Credential(error.to_string()))
    }

    pub fn project_credential_name(project: &Path) -> String {
        credential_name(project)
    }

    pub async fn read_issue(
        &self,
        project: &Path,
        issue_id: &str,
    ) -> Result<LinearIssue, LinearError> {
        let data = self
            .graphql(
                project,
                "query Issue($id: String!) { issue(id: $id) { id identifier title url state { name } } }",
                json!({ "id": issue_id }),
            )
            .await?;
        let issue = data
            .get("issue")
            .ok_or_else(|| LinearError::InvalidResponse("missing issue".into()))?;
        serde_json::from_value(json!({
            "id": issue["id"],
            "identifier": issue["identifier"],
            "title": issue["title"],
            "url": issue["url"],
            "status": issue["state"]["name"],
        }))
        .map_err(|error| LinearError::InvalidResponse(error.to_string()))
    }

    /// Update the issue state and attach a URL as a Linear link attachment.
    pub async fn write_status_and_link(
        &self,
        project: &Path,
        issue_id: &str,
        state_id: &str,
        url: &str,
        title: &str,
    ) -> Result<(), LinearError> {
        self.graphql(
            project,
            "mutation UpdateIssue($id: String!, $stateId: String!) { issueUpdate(id: $id, input: { stateId: $stateId }) { success } }",
            json!({ "id": issue_id, "stateId": state_id }),
        )
        .await?;
        self.graphql(
            project,
            "mutation AddLink($issueId: String!, $url: String!, $title: String!) { attachmentLinkURLCreate(input: { issueId: $issueId, url: $url, title: $title }) { success } }",
            json!({ "issueId": issue_id, "url": url, "title": title }),
        )
        .await?;
        Ok(())
    }

    async fn graphql(
        &self,
        project: &Path,
        query: &str,
        variables: Value,
    ) -> Result<Value, LinearError> {
        let credential_name = credential_name(project);
        let token = credentials::get_from(self.credentials.as_ref(), &credential_name).map_err(
            |error| match error {
                KeyError::Missing(_) => LinearError::MissingToken,
                other => LinearError::Credential(other.to_string()),
            },
        )?;
        let response = self
            .transport
            .post(
                &self.endpoint,
                token.expose(),
                json!({ "query": query, "variables": variables }),
            )
            .await
            .map_err(LinearError::Request)?;
        if response.status == 401 || response.status == 403 {
            return Err(reconnect_error(&credential_name));
        }
        let payload: Value = serde_json::from_str(&response.body)
            .map_err(|error| LinearError::InvalidResponse(error.to_string()))?;
        if let Some(errors) = payload.get("errors").and_then(Value::as_array) {
            let message = errors
                .iter()
                .filter_map(|error| error.get("message").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("; ");
            if is_revoked_token_message(&message) {
                return Err(reconnect_error(&credential_name));
            }
            return Err(LinearError::Request(message));
        }
        if !(200..300).contains(&response.status) {
            return Err(LinearError::Request(format!(
                "HTTP status {}",
                response.status
            )));
        }
        payload
            .get("data")
            .cloned()
            .ok_or_else(|| LinearError::InvalidResponse("missing data".into()))
    }
}

fn reconnect_error(project_key: &str) -> LinearError {
    LinearError::ReconnectRequired(ReconnectAction {
        action: "reconnect".into(),
        integration: "linear".into(),
        project_key: project_key.to_string(),
        label: "Reconnect Linear".into(),
    })
}

fn is_revoked_token_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("invalid token")
        || message.contains("token revoked")
        || message.contains("unauthorized")
        || message.contains("authentication required")
}

fn credential_name(project: &Path) -> String {
    let canonical = project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf());
    let mut hash = Sha256::new();
    hash.update(canonical.to_string_lossy().as_bytes());
    format!("linear-project-{:x}", hash.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeTransport {
        responses: Mutex<Vec<LinearResponse>>,
        calls: Mutex<Vec<(String, Value)>>,
    }

    #[async_trait]
    impl LinearTransport for FakeTransport {
        async fn post(
            &self,
            _endpoint: &str,
            token: &str,
            body: Value,
        ) -> Result<LinearResponse, String> {
            self.calls.lock().unwrap().push((token.to_string(), body));
            if self.responses.lock().unwrap().is_empty() {
                return Ok(LinearResponse {
                    status: 200,
                    body: r#"{"data":{"issue":{"id":"i-1","identifier":"RIG-1","title":"Test","url":"https://linear.app/acme/issue/RIG-1","state":{"name":"Todo"}}}}"#.into(),
                });
            }
            Ok(self.responses.lock().unwrap().remove(0))
        }
    }

    fn connector(
        credentials: Arc<dyn CredentialStore>,
        transport: Arc<FakeTransport>,
    ) -> LinearConnector {
        LinearConnector::new(credentials, transport)
    }

    #[tokio::test]
    async fn reads_issue_fields() {
        let temp = tempfile::tempdir().unwrap();
        let credentials = Arc::new(credentials::MemoryCredentialStore::with(
            &credential_name(temp.path()),
            "token-a",
        ));
        let transport = Arc::new(FakeTransport::default());
        let issue = connector(credentials, transport)
            .read_issue(temp.path(), "RIG-1")
            .await
            .unwrap();
        assert_eq!(issue.identifier, "RIG-1");
        assert_eq!(issue.title, "Test");
        assert_eq!(issue.status.as_deref(), Some("Todo"));
    }

    #[tokio::test]
    async fn writes_status_and_link_as_two_mutations() {
        let temp = tempfile::tempdir().unwrap();
        let credentials = Arc::new(credentials::MemoryCredentialStore::with(
            &credential_name(temp.path()),
            "token-a",
        ));
        let transport = Arc::new(FakeTransport::default());
        connector(credentials, transport.clone())
            .write_status_and_link(
                temp.path(),
                "issue-id",
                "state-id",
                "https://github.com/acme/repo/pull/4",
                "Review",
            )
            .await
            .unwrap();
        let calls = transport.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(
            calls[0].1["query"]
                .as_str()
                .unwrap()
                .contains("issueUpdate")
        );
        assert_eq!(calls[0].1["variables"]["stateId"], "state-id");
        assert!(
            calls[1].1["query"]
                .as_str()
                .unwrap()
                .contains("attachmentLinkURLCreate")
        );
        assert_eq!(
            calls[1].1["variables"]["url"],
            "https://github.com/acme/repo/pull/4"
        );
    }

    #[tokio::test]
    async fn tokens_are_scoped_to_the_project() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let store = Arc::new(credentials::MemoryCredentialStore::new());
        store.insert(&credential_name(first.path()), "first-token");
        store.insert(&credential_name(second.path()), "second-token");
        let transport = Arc::new(FakeTransport::default());
        let connector = connector(store.clone(), transport.clone());
        connector.read_issue(first.path(), "A-1").await.unwrap();
        connector.read_issue(second.path(), "B-1").await.unwrap();
        let calls = transport.calls.lock().unwrap();
        assert_eq!(calls[0].0, "first-token");
        assert_eq!(calls[1].0, "second-token");
        assert_ne!(
            credential_name(first.path()),
            credential_name(second.path())
        );
    }

    #[tokio::test]
    async fn revoked_token_returns_reconnect_action_without_retrying() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(credentials::MemoryCredentialStore::with(
            &credential_name(temp.path()),
            "revoked-token",
        ));
        let transport = Arc::new(FakeTransport::default());
        transport.responses.lock().unwrap().push(LinearResponse {
            status: 401,
            body: r#"{"errors":[{"message":"Unauthorized"}]}"#.into(),
        });
        let error = connector(store, transport.clone())
            .read_issue(temp.path(), "A-1")
            .await
            .unwrap_err();
        let action = error.reconnect_action().unwrap();
        assert_eq!(action.action, "reconnect");
        assert_eq!(action.integration, "linear");
        assert_eq!(action.label, "Reconnect Linear");
        assert_eq!(transport.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn recognizes_graphql_revocation_errors() {
        assert!(is_revoked_token_message("Invalid token"));
        assert!(!is_revoked_token_message("Issue not found"));
    }
}
