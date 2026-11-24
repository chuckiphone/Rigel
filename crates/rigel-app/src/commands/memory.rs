//! Project memory reads, pins and writes.
//!
//! These are the `#[tauri::command]` functions only. Shared types and startup helpers stay
//! in the crate root, which owns the window lifecycle.

use crate::*;

#[tauri::command]
pub(crate) fn memory_read(
    project: String,
    agent: String,
    topic: String,
) -> Result<Option<String>, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        MemoryService::new(Path::new(&project), service.pool().clone())
            .read(&agent, &topic)
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn memory_facts(
    project: String,
    agent: Option<String>,
) -> Result<Vec<RankedFact>, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        MemoryService::new(Path::new(&project), service.pool().clone())
            .facts(agent.as_deref())
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn memory_recall(
    project: String,
    agent: Option<String>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<RankedFact>, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        MemoryService::new(Path::new(&project), service.pool().clone())
            .recall(agent.as_deref(), &query, limit.unwrap_or(10))
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn memory_pin(project: String, id: String, pinned: bool) -> Result<RankedFact, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        MemoryService::new(Path::new(&project), service.pool().clone())
            .pin_fact(&id, pinned)
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn memory_write(
    project: String,
    agent: String,
    topic: String,
    content: String,
    mode: Option<String>,
) -> Result<MemoryFile, String> {
    let mode = mode
        .as_deref()
        .and_then(WriteMode::parse)
        .unwrap_or(WriteMode::Append);
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        let root = PathBuf::from(&project);
        MemoryService::new(&root, service.pool().clone())
            .with_approval(Arc::new(WindowMemoryApprover {
                bus: ApprovalBus::new(index_dir()),
                project_root: root,
            }))
            .write(&agent, &topic, &content, mode)
            .await
            .map_err(message)
    })
}
