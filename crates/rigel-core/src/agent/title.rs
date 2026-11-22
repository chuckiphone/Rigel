//! Cheap-model session titles (roadmap 56).
//!
//! A session should have a name before the user types one, but asking a model for that name must
//! never sit in front of the work. The generator runs after the first prompt has been accepted,
//! on the cheapest model that answers, and writes through [`SessionService::apply_auto_title`],
//! whose conditional update refuses a session the user has named. A hanging provider, a missing
//! key or a reply that is not a title costs the session nothing but that candidate.
//!
//! Candidate order is price order. The local router's Antigravity flash row is first because it
//! is already the refiner's cheap model; the OpenRouter rows behind it are the published chat
//! models with the lowest input-plus-output price in this tree, and each later row is the next
//! step up. A failed candidate falls through to the next, so "cheapest" never means "only".

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::model::{ChatMessage, ChatRequest, Delta, Model, usage_from_raw};
use super::providers;
use crate::credentials::{self, KeychainCredentialStore};
use crate::domain::Usage;
use crate::session::SessionService;

const MAX_PROMPT_CHARS: usize = 2_000;
pub const MAX_TITLE_CHARS: usize = 60;
const MAX_TITLE_WORDS: usize = 12;
/// One candidate gets this long. A title is a 24-token reply; a provider that cannot produce it
/// in twenty seconds is not working, and the next candidate should get its turn.
pub const TITLE_TIMEOUT: Duration = Duration::from_secs(20);
/// The command surface waits for the whole candidate list, including a full per-candidate
/// timeout for every row and a little room for the router to start.
pub const TITLE_TOTAL_TIMEOUT: Duration = Duration::from_secs(90);

const SYSTEM: &str = "You name coding sessions. Read the first prompt and return one title: \
two to six words, no quotes, no markdown, no trailing period. The title must describe the work \
in the prompt. Return only the title.";

/// The cheap models a title may use, cheapest first. The local router is free to this machine
/// and already runs the refiner, so it leads; the published rows behind it are ordered by their
/// combined input and output price.
pub const CHEAP_TITLE_MODELS: &[(&str, &str)] = &[
    (
        credentials::OMNIROUTE,
        "antigravity/gemini-3.8-flash-tiered",
    ),
    ("openrouter", "deepseek/deepseek-v4.1-flash"),
    ("openrouter", "openai/gpt-5-nano"),
    ("openrouter", "google/gemini-2.5-flash-lite"),
];

#[derive(Debug, Clone, Serialize)]
pub struct TitleSuggestion {
    pub title: String,
    pub provider: String,
    pub model: String,
    pub usage: Usage,
}

struct TitleCandidate {
    provider: String,
    model: String,
    client: Arc<dyn Model>,
}

/// A list of models in the order they should be tried, with a per-candidate timeout. The caller
/// supplies the clients so the app uses the keychain while tests use a scripted model.
pub struct TitleGenerator {
    candidates: Vec<TitleCandidate>,
    timeout: Duration,
}

impl TitleGenerator {
    pub fn new(candidates: Vec<(String, String, Arc<dyn Model>)>) -> Self {
        Self {
            candidates: candidates
                .into_iter()
                .map(|(provider, model, client)| TitleCandidate {
                    provider,
                    model,
                    client,
                })
                .collect(),
            timeout: TITLE_TIMEOUT,
        }
    }

    /// Shorten the per-candidate wait. Tests use this to prove a stuck candidate falls through.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Try each candidate in order. Any build, network, timeout or formatting failure is just a
    /// reason to try the next one; `None` means no cheap model produced a usable title.
    pub async fn suggest(&self, prompt: &str) -> Option<TitleSuggestion> {
        for candidate in &self.candidates {
            let ask = suggest(
                candidate.client.as_ref(),
                &candidate.provider,
                &candidate.model,
                prompt,
            );
            if let Ok(Ok(suggestion)) = tokio::time::timeout(self.timeout, ask).await {
                return Some(suggestion);
            }
        }
        None
    }
}

/// Build the generator from the providers that currently hold a credential. A provider with no
/// key is skipped here rather than turned into a failed title call.
pub async fn cheapest() -> TitleGenerator {
    let mut candidates = Vec::new();
    for (provider, model) in CHEAP_TITLE_MODELS {
        if let Ok(client) = providers::build(provider, &KeychainCredentialStore).await {
            candidates.push((provider.to_string(), model.to_string(), client));
        }
    }
    TitleGenerator::new(candidates)
}

/// The whole job: build the candidate list, ask the cheapest model that answers, and write
/// through the conditional update. A manual rename that lands in between is honoured by the
/// database, not by a check here.
pub async fn name_session(
    sessions: SessionService,
    session_id: String,
    prompt: String,
) -> Option<TitleSuggestion> {
    let generator = cheapest().await;
    let suggestion = generator.suggest(&prompt).await?;
    let _ = sessions
        .apply_auto_title(&session_id, &suggestion.title)
        .await;
    Some(suggestion)
}

/// The same job as a Tokio task. The handle is returned immediately, so the caller that created
/// the session or accepted the prompt never waits for a provider. The Tauri command does not use
/// this one: it calls [`name_session`] through `tauri::async_runtime::spawn`, because a command
/// handler is outside Tokio's runtime context.
pub fn spawn(
    sessions: SessionService,
    session_id: String,
    prompt: String,
) -> tokio::task::JoinHandle<Option<TitleSuggestion>> {
    tokio::spawn(name_session(sessions, session_id, prompt))
}

/// The same job with an already-built candidate list. The app has no use for this; tests use it
/// to hold a candidate open and prove that the caller, and the session, keep moving.
pub fn spawn_with_candidates(
    sessions: SessionService,
    session_id: String,
    prompt: String,
    candidates: Vec<(String, String, Arc<dyn Model>)>,
) -> tokio::task::JoinHandle<Option<TitleSuggestion>> {
    tokio::spawn(async move {
        let suggestion = TitleGenerator::new(candidates).suggest(&prompt).await?;
        let _ = sessions
            .apply_auto_title(&session_id, &suggestion.title)
            .await;
        Some(suggestion)
    })
}

/// One cheap call. The prompt is clipped before it leaves the machine, and the reply is reduced
/// to one line of at most [`MAX_TITLE_CHARS`] characters before it is returned.
pub async fn suggest(
    model: &dyn Model,
    provider: &str,
    model_name: &str,
    prompt: &str,
) -> Result<TitleSuggestion, String> {
    let prompt = clip(prompt.trim(), MAX_PROMPT_CHARS);
    if prompt.is_empty() {
        return Err("a session title needs a prompt".to_string());
    }

    let request = ChatRequest {
        model: model_name.to_string(),
        messages: vec![ChatMessage::system(SYSTEM), ChatMessage::user(prompt)],
        tools: Vec::new(),
        max_tokens: Some(24),
        temperature: 0.2,
        effort: None,
    };
    let cancel = CancellationToken::new();
    let mut text = String::new();
    let mut usage = Usage::default();
    model
        .chat(request, &cancel, &mut |delta| match delta {
            Delta::Text(piece) => text.push_str(&piece),
            Delta::Usage(raw) => usage = usage_from_raw(raw, model_name),
            _ => {}
        })
        .await
        .map_err(|error| error.to_string())?;

    let title = clean(&text).ok_or_else(|| "the model returned no usable title".to_string())?;
    Ok(TitleSuggestion {
        title,
        provider: provider.to_string(),
        model: model_name.to_string(),
        usage,
    })
}

/// Turn a model reply into a title. Models sometimes answer with a markdown heading or a quoted
/// line; both are removed rather than displayed. A title is one line, at most twelve words and
/// sixty characters, so it fits the session row without the row growing into its pane.
pub fn clean(raw: &str) -> Option<String> {
    let first = raw.lines().map(str::trim).find(|line| !line.is_empty())?;
    let mut line = first
        .trim_start_matches(['#', '*', '-', '`', '"', '\''])
        .trim();
    if let Some(rest) = line
        .strip_prefix("Title:")
        .or_else(|| line.strip_prefix("title:"))
    {
        line = rest.trim();
    }
    line = line.trim_matches(['"', '\'', '`']).trim();

    let words: Vec<&str> = line.split_whitespace().take(MAX_TITLE_WORDS).collect();
    let mut title = words.join(" ");
    if title.chars().count() > MAX_TITLE_CHARS {
        title = title.chars().take(MAX_TITLE_CHARS).collect();
        if let Some(space) = title.rfind(' ') {
            title.truncate(space);
        }
    }
    let title = title
        .trim_end_matches(['.', ',', ':', ';', '!', '?', '"', '\'', '`'])
        .trim()
        .to_string();
    (!title.is_empty()).then_some(title)
}

fn clip(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::model::ModelError;
    use crate::agent::pricing::{is_listed, price_for};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;
    use tokio::sync::Notify;

    struct FixedModel {
        reply: Result<&'static str, &'static str>,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Model for FixedModel {
        fn id(&self) -> String {
            "fixed".to_string()
        }

        async fn chat(
            &self,
            _request: ChatRequest,
            _cancel: &CancellationToken,
            on_delta: &mut super::super::model::DeltaSink<'_>,
        ) -> Result<(), ModelError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.reply {
                Ok(text) => {
                    on_delta(Delta::Text(text.to_string()));
                    Ok(())
                }
                Err(message) => Err(ModelError::Http(message.to_string())),
            }
        }
    }

    /// Started, then blocked until `release`. It proves the title task is not on the path that
    /// accepts the prompt.
    struct GateModel {
        started: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait]
    impl Model for GateModel {
        fn id(&self) -> String {
            "gate".to_string()
        }

        async fn chat(
            &self,
            _request: ChatRequest,
            _cancel: &CancellationToken,
            on_delta: &mut super::super::model::DeltaSink<'_>,
        ) -> Result<(), ModelError> {
            self.started.notify_one();
            self.release.notified().await;
            on_delta(Delta::Text("A name from the gate".to_string()));
            Ok(())
        }
    }

    fn candidates(
        models: Vec<(&str, &str, Arc<dyn Model>)>,
    ) -> Vec<(String, String, Arc<dyn Model>)> {
        models
            .into_iter()
            .map(|(provider, model, client)| (provider.to_string(), model.to_string(), client))
            .collect()
    }

    #[test]
    fn a_reply_becomes_one_short_line_without_markdown_or_quotes() {
        assert_eq!(
            clean("## \"Fix the Windows resume test.\"\nand more").as_deref(),
            Some("Fix the Windows resume test")
        );
        assert_eq!(
            clean("Title: `Parser cache warmup`").as_deref(),
            Some("Parser cache warmup")
        );
        assert_eq!(clean("   \n\t").as_deref(), None);
        assert_eq!(clean("\"\"\"\"\"\"").as_deref(), None);
    }

    #[test]
    fn a_long_reply_is_cut_at_a_word_boundary_and_the_character_cap() {
        let long = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi";
        let title = clean(long).unwrap();
        assert!(title.chars().count() <= MAX_TITLE_CHARS);
        assert!(title.split_whitespace().count() <= MAX_TITLE_WORDS);
        assert!(title.starts_with("alpha beta"));
        assert!(!title.ends_with(' '));
    }

    #[test]
    fn the_published_candidates_are_the_cheapest_rows_in_ascending_price() {
        assert!(
            CHEAP_TITLE_MODELS[1..]
                .iter()
                .all(|(_, model)| is_listed(model)),
            "a fallback price would hide a model that is not in the table"
        );
        let prices: Vec<f64> = CHEAP_TITLE_MODELS[1..]
            .iter()
            .map(|(_, model)| {
                let price = price_for(model);
                price.input + price.output
            })
            .collect();
        assert!(
            prices.windows(2).all(|pair| pair[0] <= pair[1]),
            "candidate prices are not ascending: {prices:?}"
        );
    }

    #[tokio::test]
    async fn the_first_working_model_wins_and_a_failed_one_falls_through() {
        let first_calls = Arc::new(AtomicUsize::new(0));
        let second_calls = Arc::new(AtomicUsize::new(0));
        let generator = TitleGenerator::new(candidates(vec![
            (
                "openrouter",
                "broken/cheap",
                Arc::new(FixedModel {
                    reply: Err("upstream is down"),
                    calls: first_calls.clone(),
                }),
            ),
            (
                "openrouter",
                "working/cheap",
                Arc::new(FixedModel {
                    reply: Ok("Fix the resume test"),
                    calls: second_calls.clone(),
                }),
            ),
        ]));

        let suggestion = generator.suggest("fix the resume test").await.unwrap();
        assert_eq!(suggestion.title, "Fix the resume test");
        assert_eq!(suggestion.model, "working/cheap");
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(second_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_hanging_candidate_times_out_and_the_next_model_answers() {
        let gate = Arc::new(GateModel {
            started: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        });
        let generator = TitleGenerator::new(candidates(vec![
            ("openrouter", "slow/cheap", gate),
            (
                "openrouter",
                "working/cheap",
                Arc::new(FixedModel {
                    reply: Ok("Fix the resume test"),
                    calls: Arc::new(AtomicUsize::new(0)),
                }),
            ),
        ]))
        .with_timeout(Duration::from_millis(40));

        let suggestion = generator.suggest("fix the resume test").await.unwrap();
        assert_eq!(suggestion.title, "Fix the resume test");
        assert_eq!(suggestion.model, "working/cheap");
    }

    #[tokio::test]
    async fn a_hanging_model_does_not_block_the_session_or_beat_a_manual_rename() {
        let tmp = TempDir::new().unwrap();
        let sessions = SessionService::open(tmp.path()).await.unwrap();
        let session = sessions.start(None, None).await.unwrap();
        assert_eq!(session.title_source, None);

        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let gate = Arc::new(GateModel {
            started: started.clone(),
            release: release.clone(),
        });

        let before = std::time::Instant::now();
        let task = spawn_with_candidates(
            sessions.clone(),
            session.id.clone(),
            "fix the Windows resume test".to_string(),
            candidates(vec![("omniroute", "cheap/gate", gate)]),
        );
        assert!(
            before.elapsed() < Duration::from_millis(200),
            "spawning the title task returned at once"
        );

        // The model is still inside its call when the user names the session.
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .expect("the title call started");
        let renamed = sessions
            .rename(&session.id, Some("My own name"))
            .await
            .unwrap();
        assert_eq!(renamed.title_source.as_deref(), Some("manual"));

        release.notify_one();
        let suggestion = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("the title task ended")
            .unwrap()
            .expect("the model produced a title");
        assert_eq!(suggestion.title, "A name from the gate");

        let stored = sessions.get(&session.id).await.unwrap();
        assert_eq!(stored.title.as_deref(), Some("My own name"));
        assert_eq!(stored.title_source.as_deref(), Some("manual"));
    }

    #[tokio::test]
    async fn an_untitled_session_receives_the_automatic_title() {
        let tmp = TempDir::new().unwrap();
        let sessions = SessionService::open(tmp.path()).await.unwrap();
        let session = sessions.start(None, None).await.unwrap();

        let suggestion = spawn_with_candidates(
            sessions.clone(),
            session.id.clone(),
            "speed up the parser".to_string(),
            candidates(vec![(
                "openrouter",
                "cheap/one",
                Arc::new(FixedModel {
                    reply: Ok("Speed up the parser"),
                    calls: Arc::new(AtomicUsize::new(0)),
                }),
            )]),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(suggestion.model, "cheap/one");

        let stored = sessions.get(&session.id).await.unwrap();
        assert_eq!(stored.title.as_deref(), Some("Speed up the parser"));
        assert_eq!(stored.title_source.as_deref(), Some("auto"));
    }
}
