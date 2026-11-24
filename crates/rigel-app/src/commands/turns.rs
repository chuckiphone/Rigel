//! Turn creation, approvals, limits, plans and spend.
//!
//! These are the `#[tauri::command]` functions only. Shared types and startup helpers stay
//! in the crate root, which owns the window lifecycle.

use crate::*;

/// Create a turn and start it. Returns as soon as the row exists; the answer streams on
/// `rigel://events`.
///
/// Two injected handles plus six caller fields. `invoke` matches arguments by name, so the
/// signature has to stay flat; `rigel-core::agent::tools` carries the same allow.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(crate) fn turn_send(
    app: tauri::AppHandle,
    state: State<'_, AppRuntime>,
    project: String,
    session: String,
    agent: String,
    prompt: String,
    model: Option<String>,
    // The provider the renderer picked *with* the model. Without it a save that has not landed
    // yet (or an agent row written on spawn) would send the model through the previous door.
    provider: Option<String>,
    effort: Option<String>,
    // Project-relative image attachments for the first user message. The renderer passes the
    // paths it imported; the command inlines them for a vision-capable adapter.
    images: Option<Vec<String>>,
) -> Result<TurnReply, String> {
    let root = PathBuf::from(&project);
    let service = tauri::async_runtime::block_on(SessionService::open(&root)).map_err(message)?;
    let stored = tauri::async_runtime::block_on(service.get(&session)).map_err(message)?;
    let images = match images {
        Some(references) => references
            .iter()
            .map(|reference| ImagePart::from_reference(&root, reference))
            .collect::<Result<Vec<_>, _>>()?,
        None => Vec::new(),
    };

    // Roadmap 56: naming is a background job. The prompt is already accepted; a hanging cheap
    // model, a missing key or a model that answers with prose cannot delay the turn, and the
    // conditional write cannot overwrite a name the user chose.
    if stored.title.is_none() {
        let title_service = service.clone();
        let title_lookup = service.clone();
        let title_session = session.clone();
        let title_id = session.clone();
        let title_prompt = prompt.clone();
        let title_app = app.clone();
        // Dropping the handle detaches the task; the prompt has already been accepted. The
        // session event updates the open header and the list without waiting for a reload.
        std::mem::drop(tauri::async_runtime::spawn(async move {
            if rigel_core::agent::title::name_session(title_service, title_session, title_prompt)
                .await
                .is_some()
                && let Ok(fresh) = title_lookup.get(&title_id).await
            {
                let _ = title_app.emit(
                    "rigel://events",
                    AgentEvent::SessionUpdated { session: fresh },
                );
            }
        }));
    }

    let settings = load_settings();

    // Roadmap 58: one ladder decides the pair the turn carries. A role preset (D94) wins, then
    // the agent row's own picker, then the composer's request, then Settings. The ladder lives
    // in the core so the window's "next turn" label and the request cannot disagree. The
    // provider travels with the row's model; an unlanded settings save cannot move it.
    let agent_row = tauri::async_runtime::block_on(service.agent(&agent)).map_err(message)?;
    let route = rigel_core::agent::routing::for_agent(
        agent_row.role.as_deref(),
        agent_row.model.as_deref(),
        agent_row.provider.as_deref(),
        agent_row.effort.as_deref(),
        model.as_deref(),
        provider.as_deref(),
        effort.as_deref(),
        &settings.model,
        &settings.provider,
        &settings.effort,
    );
    let effort = route.effort.clone();
    if !valid_effort(&effort) {
        return Err(format!(
            "unknown effort `{effort}`; use low, medium or high"
        ));
    }
    // The provider picks the default model; the effort is applied on the wire, never folded
    // into the id the provider chose. A preset or an agent row owns the model, so its door is
    // resolved from that pair; otherwise the renderer's pair is the pick. A model id can never
    // select Vertex — its namespace is also OpenRouter's spelling of Gemini.
    let requested_model = route.model.clone();
    let requested_provider = route.provider.clone();
    let explanation = explain_route(
        &settings,
        requested_provider.as_deref(),
        requested_model.as_deref(),
    )?;
    let provider = explanation.resolved.provider.clone();
    let model_name = wire_model(&explanation.resolved.model, &effort, &provider);

    // A Claude turn is not the native loop: the harness runs it and the window's approval card
    // answers its questions. The row is created here so the renderer gets it back at once, and
    // `claude::start` adopts that same row (P4.6).
    if agent_row.harness == "claude" {
        if !images.is_empty() {
            return Err(ModelError::ImagesNotSupported {
                model: model_name.clone(),
            }
            .to_string());
        }
        let turn = tauri::async_runtime::block_on(service.append_turn(&agent, Some(&prompt)))
            .map_err(message)?;
        let cap = stored.budget_cents;
        claude::start(
            app,
            claude::ClaudeTurn {
                service,
                root,
                session_id: session,
                agent: agent_row,
                turn: turn.clone(),
                prompt,
                model: model_name,
                publish_after_turn: settings.cloud_sync_sessions,
            },
        )?;
        return Ok(TurnReply {
            turn,
            budget_cents: cap,
            route: explanation,
        });
    }

    // E2E: a scripted provider replaces the network door. Everything after this point — the
    // approvals, tools, patches, transcript and bill — is the production turn loop. The seam is
    // an environment variable, so a shipped build never takes it unless a test sets it.
    // Roadmap 26: the project's OpenRouter preferences are read once, here, and travel with the
    // request. A default policy adds no field, so a project without the file sends what it did.
    let routing_policy = openrouter::RoutingPolicy::load(&root);
    let model: Arc<dyn Model> = match std::env::var_os("RIGEL_E2E_SCRIPT") {
        Some(script) => Arc::new(ScriptedModel::from_path(Path::new(&script)).map_err(message)?),
        None => {
            // A provider with no default (its roster is read live by the picker) refuses an empty
            // model id: the provider's own 400 would say less than this does.
            if model_name.trim().is_empty() {
                return Err(format!(
                    "no model chosen for `{provider}`; pick one in the model picker"
                ));
            }

            // A saved id can outlive its roster. Check it while the launch pass has it cached.
            guard_live_model(&provider, &model_name)?;

            // One row of the adapter table says where the turn goes: the key, the base URL and
            // the request profile all come from it. A local router that is down is a process to
            // launch, not a configuration error, and `build` starts it before it sends anything.
            tauri::async_runtime::block_on(providers::build_with_policy(
                &provider,
                &credentials::KeychainCredentialStore,
                Some(routing_policy),
            ))?
        }
    };

    if !images.is_empty() && !model.supports_images() {
        return Err(ModelError::ImagesNotSupported {
            model: model_name.clone(),
        }
        .to_string());
    }

    let cancels = state.cancels.clone();
    let budget = Arc::new(BudgetService::new(
        service.pool().clone(),
        cancels.clone() as Arc<dyn InterruptSink>,
    ));

    tauri::async_runtime::block_on(budget.reserve(&agent)).map_err(message)?;
    let turn = tauri::async_runtime::block_on(service.append_turn_with_images(
        &agent,
        Some(&prompt),
        &images,
    ))
    .map_err(message)?;

    // The window is the approval surface (P4.10e): a command is published to the channel the card
    // reads, and the turn waits for the user's scope. Nothing is allowed by default, and a card
    // nobody answers is a refusal.
    let tools = Tools::new(
        root.clone(),
        service.clone(),
        budget.clone(),
        index_dir(),
        StopAt::Never,
    )
    .with_scopes(Scopes::new(&root, index_dir()))
    .with_command_approval(Arc::new(WindowApprover {
        bus: ApprovalBus::new(index_dir()),
        cancels: cancels.clone(),
        session_id: session.clone(),
        agent_id: agent.clone(),
        turn_id: turn.id.clone(),
        project_root: root.clone(),
    }))
    .with_plan_approval(Arc::new(WindowApprover {
        bus: ApprovalBus::new(index_dir()),
        cancels: cancels.clone(),
        session_id: session.clone(),
        agent_id: agent.clone(),
        turn_id: turn.id.clone(),
        project_root: root.clone(),
    }))
    .with_mail_approval(Arc::new(WindowApprover {
        bus: ApprovalBus::new(index_dir()),
        cancels: cancels.clone(),
        session_id: session.clone(),
        agent_id: agent.clone(),
        turn_id: turn.id.clone(),
        project_root: root.clone(),
    }));
    let coordinator = Coordinator {
        session: service.clone(),
        budget,
        tools,
        model,
        sink: Arc::new(WindowSink { app: app.clone() }),
        cancels,
        project_root: root,
        model_name,
        effort: Some(effort),
        max_steps: usize::from(settings.max_steps),
        subagents: subagent_policy(&settings),
        depth: 0,
        parent_cancel: None,
        worktree_dir: Some(index_dir()),
        chat_parent: None,
    };

    let returned = turn.clone();
    let session_id = session;
    let agent_id = agent;
    let prompt = prompt.clone();
    let publish_after_turn = settings.cloud_sync_sessions;
    let publish_service = service.clone();
    let publish_session_id = session_id.clone();
    let cap = tauri::async_runtime::block_on(service.get(&session_id))
        .ok()
        .and_then(|stored| stored.budget_cents);

    // A native turn runs on its own thread with a current-thread runtime. It is a blocking
    // job, and keeping it off Tauri's runtime means a long model call cannot starve the
    // window's own tasks. `block_on` also sidesteps the higher-ranked future lifetime that
    // `spawn` cannot prove `Send` for (sqlx's executor bound).
    std::thread::Builder::new()
        .name("rigel-turn".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = app.emit(
                        "rigel://events",
                        AgentEvent::Error {
                            turn_id: None,
                            message: format!("could not start the turn runtime: {error}"),
                        },
                    );
                    return;
                }
            };
            if let Err(error) = runtime.block_on(coordinator.run_existing_with_images(
                &session_id,
                &agent_id,
                turn,
                &prompt,
                images,
            )) {
                let _ = app.emit(
                    "rigel://events",
                    AgentEvent::Error {
                        turn_id: None,
                        message: error.to_string(),
                    },
                );
            }
            // The switch is captured when the turn starts. A failure is logged, not shown on the
            // card: the turn itself succeeded, and the next publish can catch up.
            if publish_after_turn {
                let publish = async {
                    let cloud = rigel_core::cloud::Cloud::open(&index_dir())
                        .map_err(|error| error.to_string())?;
                    cloud
                        .publish_session(&publish_service, &publish_session_id)
                        .await
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                };
                if let Err(error) = runtime.block_on(publish) {
                    eprintln!("rigel: session not published: {error}");
                }
            }
        })
        .map_err(|error| format!("could not start the turn thread: {error}"))?;

    Ok(TurnReply {
        turn: returned,
        budget_cents: cap,
        route: explanation,
    })
}

/// D154: every open question for this project, oldest first. The card polls this after a turn
/// starts; another project's question is not this window's to answer.
#[tauri::command]
pub(crate) fn approval_pending(project: String) -> Vec<PendingApproval> {
    ApprovalBus::new(index_dir()).pending_for(Path::new(&project))
}

/// Answer one question. `None` is a refusal, which is also what an unknown scope means: an
/// unreadable answer never becomes an allow.
#[tauri::command]
pub(crate) fn approval_answer(
    id: String,
    scope: Option<String>,
    skip: Option<bool>,
    note: Option<String>,
) -> Result<(), String> {
    // A plan skip is an answer in its own right; it needs no scope.
    let skip = skip.unwrap_or(false);
    let scope = match scope.as_deref() {
        None | Some("deny") | Some("reject") => None,
        Some(_) if skip => None,
        Some(text) => match ApprovalScope::parse(text) {
            Some(scope) => Some(scope),
            None => {
                return Err(format!(
                    "`{text}` is not a scope: once, session, project, always"
                ));
            }
        },
    };
    ApprovalBus::new(index_dir()).answer_with(&id, scope, skip, note)
}

/// Tell the watcher which project to follow. Called when a session is opened.
#[tauri::command]
pub(crate) fn watch_project(project: String) {
    remember_project(Path::new(&project));
}

#[tauri::command]
pub(crate) fn turn_cancel(state: State<'_, AppRuntime>, turn: String) -> Result<(), String> {
    // Remove the card before cancelling the token: a turn blocked in an approval wait has no
    // other boundary to observe the cancellation, and a card left behind looks like Stop failed.
    ApprovalBus::new(index_dir()).deny_pending_for_turn(&turn);
    state.cancels.cancel_turn(&turn);
    Ok(())
}

/// Q254: pause at the next step boundary. The same turn row and message list are kept.
#[tauri::command]
pub(crate) fn turn_pause(state: State<'_, AppRuntime>, turn: String) -> Result<(), String> {
    state.cancels.pause_turn(&turn);
    Ok(())
}

#[tauri::command]
pub(crate) fn turn_resume(state: State<'_, AppRuntime>, turn: String) -> Result<(), String> {
    state.cancels.resume_turn(&turn);
    Ok(())
}

/// Q39: queue a user note for the running turn; it lands at the next step boundary.
#[tauri::command]
pub(crate) fn turn_steer(
    state: State<'_, AppRuntime>,
    turn: String,
    text: String,
) -> Result<bool, String> {
    Ok(state.cancels.steer(&turn, &text))
}

/// Price the first step of a turn before it is sent. Reads only: no turn row, no spend.
///
/// The model defaults to the settings model, because that is what `turn_send` will use
/// when the renderer passes no model.
#[tauri::command]
pub(crate) fn turn_estimate(
    project: String,
    session: String,
    prompt: String,
    model: Option<String>,
    provider: Option<String>,
    effort: Option<String>,
) -> Result<TurnQuote, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        let settings = load_settings();
        let route = rigel_core::agent::routing::resolve(
            &settings.provider,
            &settings.model,
            provider.as_deref(),
            model.as_deref(),
        )?;
        let effort = effort.unwrap_or_else(|| settings.effort.clone());
        let model_name = wire_model(&route.model, &effort, &route.provider);
        let context_chars = fixed_context_chars(&model_name);
        BudgetService::with_null_interrupt(service.pool().clone())
            .quote_session(&session, &model_name, context_chars, &prompt)
            .await
            .map_err(message)
    })
}

/// D98: whether this session is planning, and when a plan was last approved.
#[tauri::command]
pub(crate) fn plan_get(
    project: String,
    session: String,
) -> Result<rigel_core::plan::PlanState, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        rigel_core::plan::state(service.pool(), &session)
            .await
            .map_err(message)
    })
}

/// D98: turn plan mode on, or approve the plan by turning it off (which records the hand-off).
#[tauri::command]
pub(crate) fn plan_set(
    project: String,
    session: String,
    enabled: bool,
) -> Result<rigel_core::plan::PlanState, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        rigel_core::plan::set(service.pool(), &session, enabled)
            .await
            .map_err(message)
    })
}

/// Roadmap 42: every subagent graph in this session, with its edges and states.
#[tauri::command]
pub(crate) fn subagent_graph(
    project: String,
    session: String,
) -> Result<Vec<rigel_core::SubagentGraph>, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        service.dag(&session).await.map_err(message)
    })
}
