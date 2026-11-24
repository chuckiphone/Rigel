//! Mail and the cloud account/sync commands.
//!
//! These are the `#[tauri::command]` functions only. Shared types and startup helpers stay
//! in the crate root, which owns the window lifecycle.

use crate::*;

/// The inbox and whether a key is stored. No network call: the page has to render an
/// unconfigured app rather than fail to open.
#[tauri::command]
pub(crate) fn mail_status() -> Result<rigel_core::MailStatus, String> {
    Ok(rigel_core::Mail::status_at(&index_dir()))
}

/// Save the inbox address in `agentmail.json`. The address is not a secret.
#[tauri::command]
pub(crate) fn mail_configure(inbox: String) -> Result<rigel_core::MailStatus, String> {
    let config = rigel_core::MailConfig::new(inbox).map_err(message)?;
    rigel_core::MailConfig::save(&index_dir(), &config).map_err(message)?;
    Ok(rigel_core::Mail::status_at(&index_dir()))
}

/// The API key is written to the OS keychain and never read back into this window.
#[tauri::command]
pub(crate) fn mail_set_key(api_key: String) -> Result<rigel_core::MailStatus, String> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err("the AgentMail API key is empty".to_string());
    }
    credentials::set(credentials::AGENTMAIL, api_key).map_err(|error| error.to_string())?;
    Ok(rigel_core::Mail::status_at(&index_dir()))
}

/// The conversations in the inbox. Subjects and counts only: no body is fetched until one is
/// opened, so checking the mailbox costs a single request.
#[tauri::command]
pub(crate) async fn mail_threads(
    limit: Option<usize>,
) -> Result<Vec<rigel_core::ThreadSummary>, String> {
    let mail = rigel_core::Mail::open(&index_dir()).map_err(message)?;
    mail.threads(limit.unwrap_or(10)).await.map_err(message)
}

/// One conversation, oldest message first. This is the read path the page shows, and the one
/// that fills in any body the thread listing left out.
#[tauri::command]
pub(crate) async fn mail_thread(thread_id: String) -> Result<rigel_core::ThreadView, String> {
    let mail = rigel_core::Mail::open(&index_dir()).map_err(message)?;
    mail.thread(&thread_id).await.map_err(message)
}

/// A send the user asked for from the page. An agent's send goes through the approval card
/// instead; this is the case where the click itself is the consent.
#[tauri::command]
pub(crate) async fn mail_send(
    to: String,
    subject: String,
    text: String,
) -> Result<rigel_core::SentMail, String> {
    let mail = rigel_core::Mail::open(&index_dir()).map_err(message)?;
    mail.send(&to, &subject, &text).await.map_err(message)
}

/// The project and the account, read without a network call. An unconfigured app answers with
/// `configured: false` so the page can say what to set instead of failing to open.
#[tauri::command]
pub(crate) fn cloud_status() -> Result<rigel_core::cloud::CloudStatus, String> {
    rigel_core::cloud::Cloud::status_at(&index_dir()).map_err(message)
}

/// Save the Firebase project id and web API key in `cloud.json`. The key is public by design;
/// the Firestore rules, not this file, decide who may read a document.
#[tauri::command]
pub(crate) fn cloud_configure(
    project: String,
    api_key: String,
) -> Result<rigel_core::cloud::CloudStatus, String> {
    let config = rigel_core::cloud::CloudConfig::new(project, api_key).map_err(message)?;
    rigel_core::cloud::CloudConfig::save(&index_dir(), &config).map_err(message)?;
    rigel_core::cloud::Cloud::status_at(&index_dir()).map_err(message)
}

/// One sign-in. The password is used for this call and is never stored; only the refresh token
/// reaches the OS keychain.
#[tauri::command]
pub(crate) async fn cloud_login(
    email: String,
    password: String,
) -> Result<rigel_core::cloud::CloudStatus, String> {
    rigel_core::cloud::Cloud::open(&index_dir())
        .map_err(message)?
        .sign_in(&email, &password)
        .await
        .map_err(message)
}

/// Create the account and keep the session Firebase issues for it — a new account is signed in,
/// not sent back to the form.
#[tauri::command]
pub(crate) async fn cloud_signup(
    email: String,
    password: String,
) -> Result<rigel_core::cloud::CloudStatus, String> {
    rigel_core::cloud::Cloud::open(&index_dir())
        .map_err(message)?
        .sign_up(&email, &password)
        .await
        .map_err(message)
}

/// Ask Firebase to email a reset link. The answer is the same for an unknown address, so the
/// page cannot be used to probe for accounts.
#[tauri::command]
pub(crate) async fn cloud_reset_password(email: String) -> Result<String, String> {
    rigel_core::cloud::Cloud::open(&index_dir())
        .map_err(message)?
        .send_password_reset(&email)
        .await
        .map_err(message)
}

/// Ask Firebase to email a verification link to the signed-in account. Sending again is the
/// resend control; the link, its template and its expiry belong to Firebase.
#[tauri::command]
pub(crate) async fn cloud_send_verification() -> Result<String, String> {
    rigel_core::cloud::Cloud::open(&index_dir())
        .map_err(message)?
        .send_verification_email()
        .await
        .map_err(message)
}

/// Re-read the verification state from Firebase and keep it in the keychain session.
#[tauri::command]
pub(crate) async fn cloud_check_verification() -> Result<rigel_core::cloud::CloudStatus, String> {
    rigel_core::cloud::Cloud::open(&index_dir())
        .map_err(message)?
        .check_email_verification()
        .await
        .map_err(message)
}

/// Point App Check at the user's attestation endpoint, or turn it off with `None` or an empty
/// string. The endpoint is not secret; the tokens it returns are, and never reach the renderer.
#[tauri::command]
pub(crate) fn cloud_app_check(
    url: Option<String>,
) -> Result<rigel_core::cloud::CloudStatus, String> {
    let mut config = rigel_core::cloud::CloudConfig::load(&index_dir())
        .map_err(message)?
        .ok_or_else(|| message(rigel_core::cloud::CloudError::NotConfigured))?;
    config.app_check = url
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
        .map(|attest_url| rigel_core::cloud::AppCheckConfig { attest_url });
    rigel_core::cloud::CloudConfig::save(&index_dir(), &config).map_err(message)?;
    rigel_core::cloud::Cloud::status_at(&index_dir()).map_err(message)
}

/// Store this installation's App Check client credential. The endpoint receives it as a bearer
/// token; the renderer never reads it back, and it is not written to `cloud.json`.
#[tauri::command]
pub(crate) fn cloud_app_check_client_token(
    token: String,
) -> Result<rigel_core::cloud::CloudStatus, String> {
    let token = token.trim();
    if token.is_empty() || token.len() > 512 || token.chars().any(char::is_whitespace) {
        return Err(
            "The App Check client token is empty, too long, or contains whitespace.".to_string(),
        );
    }
    rigel_core::credentials::set(rigel_core::credentials::APP_CHECK, token)
        .map_err(|error| error.to_string())?;
    rigel_core::cloud::Cloud::status_at(&index_dir()).map_err(message)
}

#[tauri::command]
pub(crate) fn cloud_app_check_clear_client_token() -> Result<rigel_core::cloud::CloudStatus, String>
{
    rigel_core::credentials::delete(rigel_core::credentials::APP_CHECK)
        .map_err(|error| error.to_string())?;
    rigel_core::cloud::Cloud::status_at(&index_dir()).map_err(message)
}

#[tauri::command]
pub(crate) fn cloud_logout() -> Result<rigel_core::cloud::CloudStatus, String> {
    rigel_core::cloud::Cloud::open(&index_dir())
        .map_err(message)?
        .sign_out()
        .map_err(message)
}

/// Mirror the shelf's metadata into the user's project. Bodies stay in the local blob store.
///
/// Sync rather than `async fn`: the SQLite future's `&Path` borrow is not `Send` enough for the
/// command macro's boxed future, the same reason the session commands block on their runtime.
#[tauri::command]
pub(crate) fn cloud_push() -> Result<rigel_core::cloud::PushReport, String> {
    let dir = index_dir();
    tauri::async_runtime::block_on(async move {
        let cloud = rigel_core::cloud::Cloud::open(&dir).map_err(message)?;
        let index = rigel_core::SkillIndex::open(&dir).await.map_err(message)?;
        cloud.push(&index).await.map_err(message)
    })
}

/// Publish one session's prompts and assistant text to the user's Firestore project, where the
/// phone lists and reads them. Tool calls, file changes and the project path never leave here.
#[tauri::command]
pub(crate) fn cloud_publish_session(
    project: String,
    session: String,
) -> Result<rigel_core::cloud::PublishReport, String> {
    let dir = index_dir();
    tauri::async_runtime::block_on(async move {
        let cloud = rigel_core::cloud::Cloud::open(&dir).map_err(message)?;
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        cloud
            .publish_session(&service, &session)
            .await
            .map_err(message)
    })
}

/// Publish every session in the open project in one action, for a phone that should see the
/// whole history rather than only the session that happened to be open.
#[tauri::command]
pub(crate) fn cloud_publish_all(
    project: String,
) -> Result<rigel_core::cloud::PublishAllReport, String> {
    let dir = index_dir();
    tauri::async_runtime::block_on(async move {
        let cloud = rigel_core::cloud::Cloud::open(&dir).map_err(message)?;
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        cloud.publish_all(&service).await.map_err(message)
    })
}

/// Bring down rows this machine does not have. A local body is never replaced.
#[tauri::command]
pub(crate) fn cloud_pull() -> Result<rigel_core::cloud::PullReport, String> {
    let dir = index_dir();
    tauri::async_runtime::block_on(async move {
        let cloud = rigel_core::cloud::Cloud::open(&dir).map_err(message)?;
        let index = rigel_core::SkillIndex::open(&dir).await.map_err(message)?;
        cloud.pull(&index).await.map_err(message)
    })
}
