//! Swarm task queueing for the Muse coordinator.
//!
//! These are the `#[tauri::command]` functions only. Shared types and startup helpers stay
//! in the crate root, which owns the window lifecycle.

use crate::*;

/// Queue one Swarm task for Rigel, the Muse coordinator. The API holds the JSON file until Muse
/// claims it, so the window does not need to know whether Muse is connected right now. The job
/// names the project, the room agents the preflight chose, and the big-boss choice; Muse decides
/// how to split the work.
#[tauri::command]
pub(crate) fn swarm_enqueue(
    project: String,
    task: String,
    targets: Vec<String>,
    lead: Option<SwarmLead>,
) -> Result<String, String> {
    let root = PathBuf::from(project.trim());
    if !root.is_dir() {
        return Err("Open a project before starting a swarm.".to_string());
    }
    let task = task.trim().to_string();
    if task.is_empty() {
        return Err("Type the swarm task first.".to_string());
    }
    if task.chars().count() > 20_000 {
        return Err("The swarm task is longer than 20,000 characters.".to_string());
    }
    let mut unique: Vec<String> = Vec::new();
    for target in targets {
        let target = target.trim().to_string();
        if target.is_empty() || unique.iter().any(|known| known == &target) {
            continue;
        }
        if target.chars().count() > 40 || target.chars().any(char::is_control) {
            return Err(format!("{target} is not a valid room agent name"));
        }
        unique.push(target);
    }
    if unique.is_empty() {
        return Err("Pick at least one room agent for the swarm.".to_string());
    }

    let lead = match lead.unwrap_or(SwarmLead::Rigel) {
        SwarmLead::Broadcast => {
            return Err(
                "A broadcast swarm is sent directly to the room, not queued for Rigel.".to_string(),
            );
        }
        SwarmLead::Rigel => SwarmLead::Rigel,
        SwarmLead::Model { provider, model } => {
            let provider = provider.trim().to_ascii_lowercase();
            if providers::find(&provider).is_none() {
                return Err(providers::unknown_provider(&provider));
            }
            let model = model.trim().to_string();
            if model.is_empty() {
                return Err("Pick the model Rigel should use as the big boss.".to_string());
            }
            if model.chars().count() > 200 {
                return Err("The big boss model id is longer than 200 characters.".to_string());
            }
            SwarmLead::Model { provider, model }
        }
    };

    let inbox = index_dir().join("swarm").join("inbox");
    fs::create_dir_all(&inbox).map_err(message)?;
    let id = rigel_core::swarm::new_job_id();
    let body = serde_json::json!({
        "id": id,
        "project": root.to_string_lossy(),
        "prompt": task,
        "targets": unique,
        "lead": lead,
        "created_at": rigel_core::migrate::now_ms(),
    });
    let path = inbox.join(format!("{id}.json"));
    let temp = inbox.join(format!("{id}.json.tmp"));
    fs::write(&temp, body.to_string()).map_err(message)?;
    fs::rename(&temp, &path).map_err(message)?;
    // The room bridge forwards every agent's subsequent output. Queueing the job itself remains
    // local and durable; Muse is woken by the domain request the operator/bridge sends.
    Ok(id)
}
