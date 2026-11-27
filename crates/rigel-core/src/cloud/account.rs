//! Explicit export and permanent deletion for a Firebase account.
//!
//! Export walks Firestore collection ids recursively, so newly added subcollections are included
//! without updating a hard-coded list. Deletion removes documents leaf-first, then deletes the
//! Authentication user, and clears this installation's refresh token only after both succeed.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;

use super::{Cloud, CloudError, PAGE_SIZE, Session, WRITE_BATCH};

const DELETED_MARKER: &str = "cloud-account-deleted";
const REMAINS: &str = "Local Rigel data (sessions, skill bodies, attachments, backups, cloud-state.json), cloud.json, App Check settings and client credential remain; data outside this Firebase user's Firestore subtree is not removed.";

#[derive(Debug, Clone, Serialize)]
pub struct AccountExport {
    pub schema: u32,
    pub project_id: String,
    pub uid: String,
    pub email: String,
    /// Raw Firestore document resources, including their names and all fields.
    pub documents: Vec<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeletePlan {
    pub email: String,
    pub uid: String,
    pub confirmation: String,
    pub documents_to_delete: &'static str,
    pub remains: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeleteReport {
    pub project_id: String,
    pub uid: String,
    pub documents_deleted: usize,
    pub remains: &'static str,
}

#[derive(Deserialize)]
struct CollectionIds {
    #[serde(default, rename = "collectionIds")]
    ids: Vec<String>,
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
}

impl Cloud {
    /// The phrase required by [`Cloud::delete_account`] and the exact data that deletion leaves.
    pub async fn delete_plan(&self) -> Result<DeletePlan, CloudError> {
        let session = self.token().await?;
        Ok(plan_for(&session.email, &session.uid))
    }

    /// Export the complete account subtree, recursively including every nested collection.
    pub async fn export_account(&self) -> Result<AccountExport, CloudError> {
        let session = self.token().await?;
        let root = account_document(&self.config.project_id, &session.uid);
        let mut documents = Vec::new();
        if let Some(document) = self.get_document_optional(&session, &root).await? {
            documents.push(document);
        }
        let mut descendants = self.walk_document(&session, &root).await?;
        documents.append(&mut descendants);
        documents.sort_by_key(resource_name);
        Ok(AccountExport {
            schema: 1,
            project_id: self.config.project_id.clone(),
            uid: session.uid,
            email: session.email,
            documents,
        })
    }

    /// Delete every Firestore document and the Firebase Authentication user.
    ///
    /// The caller must first show [`Cloud::delete_plan`] and pass its exact `confirmation` value.
    /// The phrase includes the account email, preventing an accidental confirmation from a stale
    /// dialog or a different signed-in account.
    pub async fn delete_account(&self, confirmation: &str) -> Result<DeleteReport, CloudError> {
        let session = self.token().await?;
        let plan = plan_for(&session.email, &session.uid);
        if confirmation != plan.confirmation {
            return Err(CloudError::Input(format!(
                "account deletion requires the exact confirmation `{}`; {}",
                plan.confirmation, plan.remains
            )));
        }

        let root = account_document(&self.config.project_id, &session.uid);
        let mut documents = self.walk_document(&session, &root).await?;
        if let Some(document) = self.get_document_optional(&session, &root).await? {
            documents.push(document);
        }
        let mut names = documents
            .iter()
            .filter_map(resource_name)
            .collect::<Vec<_>>();
        names.sort_by_key(|name| std::cmp::Reverse(name.matches('/').count()));
        for batch in names.chunks(WRITE_BATCH) {
            let writes: Vec<Value> = batch.iter().map(|name| json!({ "delete": name })).collect();
            let url = format!(
                "{}/v1/projects/{}/databases/(default)/documents:batchWrite",
                self.endpoints.firestore, self.config.project_id
            );
            let result: Value = self
                .post_json_authorized(&url, &json!({ "writes": writes }), &session.id_token)
                .await?;
            let error = result
                .get("status")
                .and_then(Value::as_array)
                .and_then(|statuses| {
                    statuses.iter().find(|status| {
                        status
                            .get("code")
                            .and_then(Value::as_i64)
                            .is_some_and(|code| code != 0)
                    })
                });
            if let Some(error) = error {
                return Err(CloudError::Malformed(format!(
                    "Firestore deletion batch failed: {error}"
                )));
            }
        }

        let _: Value = self
            .post_json_authorized(
                &self.identity_url("accounts:delete"),
                &json!({ "idToken": session.id_token }),
                &session.id_token,
            )
            .await?;
        self.account_deleted
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(dir) = &self.state_dir {
            std::fs::write(dir.join(DELETED_MARKER), b"deleted\n")?;
        }
        self.store.clear()?;
        Ok(DeleteReport {
            project_id: self.config.project_id.clone(),
            uid: session.uid,
            documents_deleted: names.len(),
            remains: REMAINS,
        })
    }

    fn walk_document<'a>(
        &'a self,
        session: &'a Session,
        document: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Value>, CloudError>> + Send + 'a>> {
        Box::pin(async move {
            let mut collections = Vec::new();
            let mut page_token: Option<String> = None;
            loop {
                let url = format!(
                    "{}/v1/projects/{}/databases/(default)/documents/{}:listCollectionIds",
                    self.endpoints.firestore,
                    self.config.project_id,
                    api_document_path(document)
                );
                let mut body = json!({ "pageSize": PAGE_SIZE });
                if let Some(token) = &page_token {
                    body["pageToken"] = json!(token);
                }
                let page: CollectionIds = self
                    .post_json_authorized(&url, &body, &session.id_token)
                    .await?;
                collections.extend(page.ids);
                page_token = page.next_page_token;
                if page_token.is_none() {
                    break;
                }
            }

            let mut output = Vec::new();
            for collection in collections {
                let path = format!("{document}/{collection}");
                let mut page_token: Option<String> = None;
                loop {
                    let url = format!(
                        "{}/v1/projects/{}/databases/(default)/documents/{}",
                        self.endpoints.firestore,
                        self.config.project_id,
                        api_document_path(&path)
                    );
                    let mut request = self
                        .http
                        .get(&url)
                        .bearer_auth(&session.id_token)
                        .query(&[("pageSize", PAGE_SIZE.to_string())]);
                    if let Some(token) = &page_token {
                        request = request.query(&[("pageToken", token)]);
                    }
                    if let Some(token) = self.app_check_token().await? {
                        request = request.header("X-Firebase-AppCheck", token);
                    }
                    let page: super::ListAnswer = self.decode(request.send().await?).await?;
                    for item in page.documents {
                        if let Some(name) = resource_name(&item) {
                            output.push(item);
                            output.extend(self.walk_document(session, &name).await?);
                        }
                    }
                    page_token = page.next_page_token;
                    if page_token.is_none() {
                        break;
                    }
                }
            }
            Ok(output)
        })
    }

    async fn get_document_optional(
        &self,
        session: &Session,
        document: &str,
    ) -> Result<Option<Value>, CloudError> {
        let url = format!(
            "{}/v1/projects/{}/databases/(default)/documents/{}",
            self.endpoints.firestore,
            self.config.project_id,
            api_document_path(document)
        );
        let mut request = self.http.get(url).bearer_auth(&session.id_token);
        if let Some(token) = self.app_check_token().await? {
            request = request.header("X-Firebase-AppCheck", token);
        }
        let response = request.send().await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        self.decode(response).await.map(Some)
    }
}

fn api_document_path(resource_name: &str) -> &str {
    resource_name
        .split_once("/documents/")
        .map_or(resource_name, |(_, path)| path)
}

fn account_document(project: &str, uid: &str) -> String {
    format!("projects/{project}/databases/(default)/documents/users/{uid}")
}

fn resource_name(document: &Value) -> Option<String> {
    document.get("name")?.as_str().map(str::to_owned)
}

fn plan_for(email: &str, uid: &str) -> DeletePlan {
    DeletePlan {
        email: email.to_owned(),
        uid: uid.to_owned(),
        confirmation: format!("DELETE {email}"),
        documents_to_delete: "all Firestore documents under users/{uid}, followed by the Firebase Authentication account",
        remains: REMAINS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use super::super::{CloudConfig, Endpoints, MemorySessionStore, Session, SessionStore};

    #[test]
    fn plan_requires_account_specific_typed_confirmation_and_names_remaining_data() {
        let plan = plan_for("person@example.test", "uid-1");
        assert_eq!(plan.confirmation, "DELETE person@example.test");
        assert!(plan.documents_to_delete.contains("all Firestore documents"));
        assert!(plan.remains.contains("Local Rigel data"));
        assert!(
            plan.remains
                .contains("outside this Firebase user's Firestore subtree")
        );
    }

    #[test]
    fn export_keeps_every_document_resource_not_only_known_fields() {
        let docs = [
            json!({"name":"users/u/sessions/s/turns/t", "fields":{"future":{"stringValue":"kept"}}}),
            json!({"name":"users/u/skills/a", "fields":{"id":{"stringValue":"skill"}}}),
        ];
        let mut docs = docs.to_vec();
        docs.sort_by_key(resource_name);
        assert_eq!(docs.len(), 2);
        assert!(
            docs.iter()
                .any(|doc| doc["fields"]["future"]["stringValue"] == "kept")
        );
        assert!(
            docs.iter()
                .any(|doc| resource_name(doc).as_deref() == Some("users/u/skills/a"))
        );
    }

    #[test]
    fn deleted_account_has_a_stable_documented_not_found_error() {
        assert_eq!(
            CloudError::AccountNotFound.to_string(),
            "cloud account not found: this account was deleted; sign in to another account"
        );
    }

    fn cloud_with_session() -> (Cloud, Arc<MemorySessionStore>) {
        let store = Arc::new(MemorySessionStore::new());
        store
            .save(&Session {
                uid: "uid-1".into(),
                email: "person@example.test".into(),
                project_id: "rigel-cloud".into(),
                id_token: "id-token".into(),
                refresh_token: "refresh-token".into(),
                expires_at: super::super::now_ms() + Duration::from_secs(3600).as_millis() as i64,
                email_verified: true,
            })
            .expect("save test session");
        let cloud = Cloud::new(
            CloudConfig {
                project_id: "rigel-cloud".into(),
                api_key: "public-key".into(),
                region: None,
                app_check: None,
                push: None,
            },
            Endpoints::same("http://127.0.0.1:1"),
            store.clone(),
        )
        .expect("construct test client");
        (cloud, store)
    }

    #[tokio::test]
    async fn wrong_confirmation_is_rejected_before_any_cloud_request() {
        let (cloud, store) = cloud_with_session();
        let error = cloud
            .delete_account("DELETE someone-else@example.test")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("DELETE person@example.test"));
        assert!(error.to_string().contains("Local Rigel data"));
        assert!(
            store.account().is_some(),
            "confirmation failure must not clear local credentials"
        );
    }

    #[tokio::test]
    async fn requests_after_deletion_return_documented_not_found() {
        let (cloud, store) = cloud_with_session();
        cloud
            .account_deleted
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let error = cloud.export_account().await.unwrap_err();
        assert!(matches!(error, CloudError::AccountNotFound));
        assert!(error.to_string().contains("account was deleted"));
        assert!(store.account().is_some());
    }

    #[tokio::test]
    async fn confirmed_delete_removes_documents_clears_credentials_and_persists_tombstone() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            for _ in 0..4 {
                let (mut stream, _) = listener.accept().expect("accept request");
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .expect("set read timeout");
                let mut request = Vec::new();
                let mut chunk = [0u8; 4096];
                let body_start;
                loop {
                    let count = stream.read(&mut chunk).expect("read request");
                    request.extend_from_slice(&chunk[..count]);
                    if let Some(index) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        body_start = index + 4;
                        break;
                    }
                }
                let headers = String::from_utf8_lossy(&request);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                while request.len() < body_start + content_length {
                    let count = stream.read(&mut chunk).expect("read request body");
                    request.extend_from_slice(&chunk[..count]);
                }
                let first_line = String::from_utf8_lossy(&request)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let target = first_line.split_whitespace().nth(1).unwrap_or_default();
                let response = if target.ends_with("users/uid-1:listCollectionIds") {
                    json!({"collectionIds": []})
                } else if first_line.starts_with("GET ") {
                    json!({"name":"projects/rigel-cloud/databases/(default)/documents/users/uid-1","fields":{"email":{"stringValue":"person@example.test"}}})
                } else if target.contains("documents:batchWrite") {
                    requests.push(String::from_utf8_lossy(&request).into_owned());
                    json!({"writeResults":[{}],"status":[{"code":0}]})
                } else if target.contains("accounts:delete") {
                    json!({})
                } else {
                    panic!("unexpected request: {first_line}");
                };
                let body = serde_json::to_vec(&response).expect("encode response");
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).expect("write response headers");
                stream.write_all(&body).expect("write response body");
            }
            requests
        });
        let (mut cloud, store) = cloud_with_session();
        let directory = tempfile::tempdir().expect("state directory");
        cloud.state_dir = Some(directory.path().to_path_buf());
        cloud.endpoints = Endpoints::same(format!("http://{address}"));

        let report = cloud
            .delete_account("DELETE person@example.test")
            .await
            .expect("confirmed account deletion");
        let requests = server.join().expect("fixture server exits");
        assert_eq!(report.documents_deleted, 1);
        assert!(report.remains.contains("Local Rigel data"));
        assert!(
            report
                .remains
                .contains("outside this Firebase user's Firestore subtree")
        );
        assert!(
            requests[0].contains("projects/rigel-cloud/databases/(default)/documents/users/uid-1")
        );
        assert!(store.account().is_none());
        assert!(directory.path().join(DELETED_MARKER).is_file());
        assert!(matches!(
            cloud.export_account().await,
            Err(CloudError::AccountNotFound)
        ));
        let reopened = Cloud::new_at(
            directory.path(),
            CloudConfig {
                project_id: "rigel-cloud".into(),
                api_key: "public-key".into(),
                region: None,
                app_check: None,
                push: None,
            },
            Endpoints::same("http://127.0.0.1:1"),
            Arc::new(MemorySessionStore::new()),
        )
        .expect("reopen cloud client");
        assert!(matches!(
            reopened.export_account().await,
            Err(CloudError::AccountNotFound)
        ));
    }

    #[tokio::test]
    async fn export_walks_account_skills_sessions_and_nested_turns() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        listener
            .set_nonblocking(true)
            .expect("nonblocking fixture listener");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            for _ in 0..8 {
                let deadline = std::time::Instant::now() + Duration::from_secs(8);
                let (mut stream, _) = loop {
                    match listener.accept() {
                        Ok(connection) => break connection,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && std::time::Instant::now() < deadline =>
                        {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(error) => panic!("fixture server accept failed: {error}"),
                    }
                };
                stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
                let mut request = Vec::new();
                let mut chunk = [0u8; 4096];
                let body_start;
                loop {
                    let count = stream.read(&mut chunk).expect("read request");
                    request.extend_from_slice(&chunk[..count]);
                    if let Some(index) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        body_start = index + 4;
                        break;
                    }
                }
                let headers = String::from_utf8_lossy(&request);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                while request.len() < body_start + content_length {
                    let count = stream.read(&mut chunk).expect("read request body");
                    request.extend_from_slice(&chunk[..count]);
                }
                let first_line = String::from_utf8_lossy(&request)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let target = first_line.split_whitespace().nth(1).unwrap_or_default();
                let response = if first_line.starts_with("GET ")
                    && target.ends_with("/documents/users/uid-1")
                {
                    json!({"name":"projects/rigel-cloud/databases/(default)/documents/users/uid-1","fields":{"email":{"stringValue":"person@example.test"}}})
                } else if target.ends_with("users/uid-1:listCollectionIds") {
                    json!({"collectionIds":["sessions","skills"]})
                } else if target.ends_with("sessions/s-1:listCollectionIds") {
                    json!({"collectionIds":["turns"]})
                } else if target.ends_with("turns/t-1:listCollectionIds")
                    || target.ends_with("skills/k-1:listCollectionIds")
                {
                    json!({"collectionIds":[]})
                } else if first_line.starts_with("GET ") && target.contains("/sessions?") {
                    json!({"documents":[{"name":"projects/rigel-cloud/databases/(default)/documents/users/uid-1/sessions/s-1","fields":{"title":{"stringValue":"session"}}}]})
                } else if first_line.starts_with("GET ") && target.contains("/turns?") {
                    json!({"documents":[{"name":"projects/rigel-cloud/databases/(default)/documents/users/uid-1/sessions/s-1/turns/t-1","fields":{"prompt":{"stringValue":"prompt"},"future":{"stringValue":"preserved"}}}]})
                } else if first_line.starts_with("GET ") && target.contains("/skills?") {
                    json!({"documents":[{"name":"projects/rigel-cloud/databases/(default)/documents/users/uid-1/skills/k-1","fields":{"id":{"stringValue":"skill"}}}]})
                } else {
                    json!({"collectionIds":[]})
                };
                let body = serde_json::to_vec(&response).expect("encode fixture response");
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).expect("write response headers");
                stream.write_all(&body).expect("write response body");
            }
        });
        let (mut cloud, _) = cloud_with_session();
        cloud.endpoints = Endpoints::same(format!("http://{address}"));
        let export = cloud
            .export_account()
            .await
            .expect("export complete account");
        server.join().expect("fixture server exits");
        assert_eq!(export.documents.len(), 4);
        assert!(
            export
                .documents
                .iter()
                .any(|doc| resource_name(doc).is_some_and(|name| name.ends_with("/turns/t-1")))
        );
        assert!(
            export
                .documents
                .iter()
                .any(|doc| doc["fields"]["future"]["stringValue"] == "preserved")
        );
    }
}
