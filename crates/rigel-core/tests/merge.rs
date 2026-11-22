//! The conflict view's three-way merge (D79).
//!
//! What is asserted here is the promise the Diff view makes: the three versions are captured with
//! the hashes the ledger recorded, the merge is computed before anything is written, and a
//! resolution writes the merged file through the same compare-and-swap path as an agent patch.

use rigel_core::{
    ClaimError, ClaimService, ConflictHunkDecision, ConflictVerdictInput, MERGE_AUTHOR,
    MergedConflict, PatchError, PatchPipeline, SessionService,
};

const BASE: &str = "one\ntwo\nthree\nfour\nfive\n";

struct Fixture {
    tmp: tempfile::TempDir,
    claims: ClaimService,
    pipeline: PatchPipeline,
    session: String,
    claimant: String,
    writer: String,
}

impl Fixture {
    /// A project with one claimed file and two agents: the claimant, which writes first, and the
    /// writer, which writes over it.
    async fn new(path: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(path), BASE).unwrap();
        let sessions = SessionService::open(tmp.path()).await.unwrap();
        let session = sessions.start(Some("merge"), None).await.unwrap();
        let claimant = sessions
            .register_agent(&session.id, "native", Some("worker"))
            .await
            .unwrap();
        let writer = sessions
            .register_agent(&session.id, "native", Some("reviewer"))
            .await
            .unwrap();
        let claims = ClaimService::new(sessions.pool().clone(), tmp.path());
        let pipeline = PatchPipeline::new(tmp.path(), sessions.pool().clone());
        claims
            .acquire(&claimant.id, &[path.to_string()], "edit it")
            .await
            .unwrap();
        Self {
            tmp,
            claims,
            pipeline,
            session: session.id,
            claimant: claimant.id,
            writer: writer.id,
        }
    }

    fn path(&self, path: &str) -> std::path::PathBuf {
        self.tmp.path().join(path)
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.path(path)).unwrap()
    }

    fn hash(&self, text: &str) -> String {
        rigel_core::patches::sha256_hex(text.as_bytes())
    }

    /// The reviewer answers every conflicted hunk. The tests choose `ours` unless they are
    /// deliberately exercising a bad verdict; the core checks coverage and the reviewer, not the
    /// visual choice.
    fn verdict(&self, merged: &MergedConflict, reviewer: &str) -> ConflictVerdictInput {
        ConflictVerdictInput {
            reviewer: reviewer.to_string(),
            decisions: (0..merged.hunks.len())
                .map(|index| ConflictHunkDecision {
                    index,
                    choice: "ours".to_string(),
                })
                .collect(),
            test_hint: "cargo test -p rigel-core --test merge".to_string(),
            tests_acknowledged: true,
        }
    }

    async fn write_as(&self, agent: &str, path: &str, text: &str) -> String {
        let (_, base) = self.pipeline.read(path).unwrap();
        self.pipeline
            .propose(&self.session, agent, path, &base, text)
            .await
            .unwrap();
        self.hash(text)
    }

    /// Replace the file without the patch pipeline: the writer an adapter or an editor would be.
    fn write_raw(&self, path: &str, text: &str) -> String {
        std::fs::write(self.path(path), text).unwrap();
        self.hash(text)
    }
}

#[tokio::test]
async fn the_three_sides_are_captured_with_the_recorded_hashes() {
    let path = "notes.txt";
    let fixture = Fixture::new(path).await;
    let base_hash = fixture.hash(BASE);

    let ours_text = BASE.replace("two\n", "TWO\n");
    let ours_hash = fixture.write_as(&fixture.claimant, path, &ours_text).await;
    let theirs_text = ours_text.replace("four\n", "FOUR\n");
    let theirs_hash = fixture.write_as(&fixture.writer, path, &theirs_text).await;

    let conflict = fixture
        .claims
        .record_conflict(
            path,
            Some(&fixture.claimant),
            Some(&fixture.writer),
            Some(base_hash.clone()),
            Some(theirs_hash.clone()),
        )
        .await
        .unwrap();

    let sides = fixture.claims.sides(&conflict.id).await.unwrap();
    assert_eq!(
        sides.base.as_ref().map(|side| side.hash.clone()),
        Some(base_hash)
    );
    assert_eq!(
        sides.base.as_ref().map(|side| side.text.clone()),
        Some(BASE.to_string())
    );
    assert_eq!(
        sides.ours.as_ref().map(|side| side.hash.clone()),
        Some(ours_hash)
    );
    assert_eq!(
        sides.ours.as_ref().map(|side| side.text.clone()),
        Some(ours_text)
    );
    assert_eq!(
        sides.theirs.as_ref().map(|side| side.hash.clone()),
        Some(theirs_hash)
    );
    assert_eq!(
        sides.theirs.as_ref().map(|side| side.text.clone()),
        Some(theirs_text)
    );
    assert_eq!(sides.note, None, "every side was rebuilt from the journal");
}

#[tokio::test]
async fn a_write_outside_the_pipeline_is_merged_with_the_claimants_version() {
    let path = "notes.txt";
    let fixture = Fixture::new(path).await;
    let base_hash = fixture.hash(BASE);
    let ours_text = BASE.replace("two\n", "TWO\n");
    fixture.write_as(&fixture.claimant, path, &ours_text).await;

    // An editor or an adapter writes over the claimant's work without a patch row. Its content
    // is based on the claim, so the claimant's edit is the part that would be lost.
    let theirs_text = format!("{BASE}appended by the writer\n");
    let theirs_hash = fixture.write_raw(path, &theirs_text);

    let conflict = fixture
        .claims
        .record_conflict(
            path,
            Some(&fixture.claimant),
            None,
            Some(base_hash),
            Some(theirs_hash),
        )
        .await
        .unwrap();

    let sides = fixture.claims.sides(&conflict.id).await.unwrap();
    assert_eq!(
        sides.base.as_ref().map(|side| side.text.clone()),
        Some(BASE.to_string()),
        "the claim captured the version it started from"
    );
    assert_eq!(
        sides.ours.as_ref().map(|side| side.text.clone()),
        Some(ours_text.clone()),
        "ours is the claimant's patch, not the file"
    );
    assert_eq!(
        sides.theirs.as_ref().map(|side| side.text.clone()),
        Some(theirs_text.clone())
    );
    assert_eq!(sides.note, None);

    let merged = fixture.claims.merge(&conflict.id).await.unwrap();
    assert!(merged.clean, "{merged:?}");
    assert_eq!(merged.conflicts, 0);
    assert_eq!(
        merged.merged, "one\nTWO\nthree\nfour\nfive\nappended by the writer\n",
        "both sides survive the merge"
    );

    let resolution = fixture
        .claims
        .resolve_with_merge(
            &fixture.pipeline,
            &fixture.session,
            &conflict.id,
            &merged.merged,
            &fixture.verdict(&merged, "Ada Reviewer"),
        )
        .await
        .unwrap();
    assert!(!resolution.unchanged);
    let patch = resolution
        .patch
        .expect("the merged text differs from the file");
    assert_eq!(patch.status, "applied");
    assert_eq!(patch.agent_id, MERGE_AUTHOR, "a merge is the human's write");
    assert_eq!(patch.path, path);
    assert_eq!(fixture.read(path), merged.merged);

    let sides = fixture.claims.sides(&conflict.id).await.unwrap();
    assert_eq!(sides.merged_patch_id.as_deref(), Some(patch.id.as_str()));
    assert_eq!(
        sides.merged_hash.as_deref(),
        Some(patch.result_hash.as_str())
    );

    let row = fixture.claims.conflict(&conflict.id).await.unwrap();
    assert!(row.resolved_at.is_some());
    assert_eq!(row.resolution.as_deref(), Some("merged the three versions"));

    // The patch is in the session's ledger, attributed to `user`; the Diff view suppresses the
    // comment box for it because there is no agent to send a comment to.
    let hunks = fixture
        .pipeline
        .attributed_hunks(&fixture.session)
        .await
        .unwrap();
    assert!(
        hunks
            .iter()
            .any(|hunk| hunk.patch_id == patch.id && hunk.agent_id == MERGE_AUTHOR),
        "{hunks:?}"
    );

    // Resolving again is refused rather than rewriting the file.
    assert!(matches!(
        fixture
            .claims
            .resolve_with_merge(
                &fixture.pipeline,
                &fixture.session,
                &conflict.id,
                "again\n",
                &fixture.verdict(&merged, "Ada Reviewer"),
            )
            .await,
        Err(ClaimError::ConflictClosed(_))
    ));
}

#[tokio::test]
async fn a_linear_writer_that_built_on_ours_needs_no_write() {
    let path = "notes.txt";
    let fixture = Fixture::new(path).await;
    let base_hash = fixture.hash(BASE);
    let ours_text = BASE.replace("two\n", "TWO\n");
    fixture.write_as(&fixture.claimant, path, &ours_text).await;
    let theirs_text = ours_text.replace("four\n", "FOUR\n");
    let theirs_hash = fixture.write_as(&fixture.writer, path, &theirs_text).await;

    let conflict = fixture
        .claims
        .record_conflict(
            path,
            Some(&fixture.claimant),
            Some(&fixture.writer),
            Some(base_hash),
            Some(theirs_hash),
        )
        .await
        .unwrap();

    let merged = fixture.claims.merge(&conflict.id).await.unwrap();
    assert!(merged.clean, "{merged:?}");
    assert_eq!(merged.merged, theirs_text, "the writer built on our change");

    let resolution = fixture
        .claims
        .resolve_with_merge(
            &fixture.pipeline,
            &fixture.session,
            &conflict.id,
            &merged.merged,
            &fixture.verdict(&merged, "Ada Reviewer"),
        )
        .await
        .unwrap();
    assert!(resolution.unchanged, "nothing to write");
    assert!(resolution.patch.is_none());
    assert_eq!(fixture.read(path), theirs_text);
    let row = fixture.claims.conflict(&conflict.id).await.unwrap();
    assert!(row.resolved_at.is_some());
}

#[tokio::test]
async fn unresolved_markers_are_refused_and_the_next_write_records_the_reviewer() {
    let path = "notes.txt";
    let fixture = Fixture::new(path).await;
    let base_hash = fixture.hash(BASE);
    let ours_text = BASE.replace("three\n", "THREE from ours\n");
    fixture.write_as(&fixture.claimant, path, &ours_text).await;
    let theirs_text = ours_text.replace("THREE from ours\n", "THREE from theirs\n");
    let theirs_hash = fixture.write_as(&fixture.writer, path, &theirs_text).await;

    let conflict = fixture
        .claims
        .record_conflict(
            path,
            Some(&fixture.claimant),
            Some(&fixture.writer),
            Some(base_hash),
            Some(theirs_hash),
        )
        .await
        .unwrap();

    let merged = fixture.claims.merge(&conflict.id).await.unwrap();
    assert_eq!(merged.conflicts, 1);
    assert!(!merged.clean);
    assert!(merged.merged.contains("<<<<<<< ours"));
    assert!(merged.merged.contains("THREE from ours"));
    assert!(merged.merged.contains("======="));
    assert!(merged.merged.contains("THREE from theirs"));
    assert!(merged.merged.contains(">>>>>>> theirs"));

    // A marker block is the question, not an answer: even a complete verdict cannot write it.
    let refused = fixture
        .claims
        .resolve_with_merge(
            &fixture.pipeline,
            &fixture.session,
            &conflict.id,
            &merged.merged,
            &fixture.verdict(&merged, "Ada Reviewer"),
        )
        .await;
    assert!(matches!(refused, Err(ClaimError::UnresolvedMarkers)));
    assert_eq!(fixture.read(path), theirs_text, "nothing was written");

    let resolved = "one\ntwo\nTHREE from ours\nfour\nfive\n";
    let resolution = fixture
        .claims
        .resolve_with_merge(
            &fixture.pipeline,
            &fixture.session,
            &conflict.id,
            resolved,
            &fixture.verdict(&merged, "Ada Reviewer"),
        )
        .await
        .unwrap();
    assert_eq!(resolution.verdict.reviewer, "Ada Reviewer");
    assert_eq!(resolution.verdict.decisions.len(), 1);
    assert_eq!(
        resolution.verdict.test_hint,
        "cargo test -p rigel-core --test merge"
    );
    assert!(resolution.verdict.tests_acknowledged);
    assert_eq!(fixture.read(path), resolved);

    let sides = fixture.claims.sides(&conflict.id).await.unwrap();
    let verdict = sides
        .verdict
        .expect("the verdict is stored with the conflict");
    assert_eq!(verdict.reviewer, "Ada Reviewer");
    assert_eq!(verdict.verdict, "accepted");
    assert_eq!(verdict.merged_hash, resolution.patch.unwrap().result_hash);
}

#[tokio::test]
async fn a_claim_without_captured_text_falls_back_to_the_ledger() {
    let path = "notes.txt";
    let fixture = Fixture::new(path).await;
    let base_hash = fixture.hash(BASE);

    // A claim recorded before 0010 has no base text. Its conflict can still be resolved, but
    // there is nothing to merge from when the write did not come through the journal.
    sqlx::query("UPDATE claims SET base_text = NULL")
        .execute(fixture.claims.pool())
        .await
        .unwrap();
    let observed = "written by something else\n";
    let observed_hash = fixture.write_raw(path, observed);

    let conflict = fixture
        .claims
        .record_conflict(
            path,
            Some(&fixture.claimant),
            None,
            Some(base_hash),
            Some(observed_hash),
        )
        .await
        .unwrap();

    let sides = fixture.claims.sides(&conflict.id).await.unwrap();
    assert_eq!(
        sides.theirs.as_ref().map(|side| side.text.clone()),
        Some(observed.to_string())
    );
    assert!(sides.ours.is_none());
    assert!(sides.base.is_none());
    let note = sides.note.unwrap_or_default();
    assert!(note.contains("not in the patch journal"), "{note:?}");

    assert!(matches!(
        fixture.claims.merge(&conflict.id).await,
        Err(ClaimError::SidesUnavailable(_))
    ));
}

#[tokio::test]
async fn a_write_after_the_capture_is_refused_instead_of_overwritten() {
    let path = "notes.txt";
    let fixture = Fixture::new(path).await;
    let base_hash = fixture.hash(BASE);
    let ours_text = BASE.replace("two\n", "TWO\n");
    fixture.write_as(&fixture.claimant, path, &ours_text).await;
    let theirs_text = ours_text.replace("four\n", "FOUR\n");
    let theirs_hash = fixture.write_as(&fixture.writer, path, &theirs_text).await;

    let conflict = fixture
        .claims
        .record_conflict(
            path,
            Some(&fixture.claimant),
            Some(&fixture.writer),
            Some(base_hash),
            Some(theirs_hash),
        )
        .await
        .unwrap();
    let merged = fixture.claims.merge(&conflict.id).await.unwrap();

    // A third writer lands between the merge preview and the write.
    let after = "one\nTWO\nthree\nFOUR\nfive\nsix\n";
    std::fs::write(fixture.path(path), after).unwrap();

    let outcome = fixture
        .claims
        .resolve_with_merge(
            &fixture.pipeline,
            &fixture.session,
            &conflict.id,
            &merged.merged,
            &fixture.verdict(&merged, "Ada Reviewer"),
        )
        .await;
    assert!(
        matches!(
            outcome,
            Err(ClaimError::Patch(PatchError::StaleBase { .. }))
        ),
        "{outcome:?}"
    );
    assert_eq!(fixture.read(path), after, "nothing was overwritten");
    let row = fixture.claims.conflict(&conflict.id).await.unwrap();
    assert!(
        row.resolved_at.is_none(),
        "a refused write leaves the row open"
    );
}

#[tokio::test]
async fn a_claimant_that_never_wrote_has_base_equal_to_ours() {
    let path = "notes.txt";
    let fixture = Fixture::new(path).await;
    let base_hash = fixture.hash(BASE);

    // Only the writer edits; the claimant only claimed.
    let theirs_text = BASE.replace("five\n", "FIVE\n");
    let theirs_hash = fixture.write_as(&fixture.writer, path, &theirs_text).await;
    let conflict = fixture
        .claims
        .record_conflict(
            path,
            Some(&fixture.claimant),
            Some(&fixture.writer),
            Some(base_hash),
            Some(theirs_hash),
        )
        .await
        .unwrap();

    let sides = fixture.claims.sides(&conflict.id).await.unwrap();
    assert_eq!(
        sides.base.as_ref().map(|side| side.text.clone()),
        Some(BASE.to_string())
    );
    assert_eq!(
        sides.ours.as_ref().map(|side| side.text.clone()),
        Some(BASE.to_string())
    );

    let merged = fixture.claims.merge(&conflict.id).await.unwrap();
    assert!(merged.clean);
    assert_eq!(merged.merged, theirs_text);

    // The merge is what the file already holds: no patch, but the row is answered.
    let resolution = fixture
        .claims
        .resolve_with_merge(
            &fixture.pipeline,
            &fixture.session,
            &conflict.id,
            &merged.merged,
            &fixture.verdict(&merged, "Ada Reviewer"),
        )
        .await
        .unwrap();
    assert!(resolution.unchanged);
    assert!(resolution.patch.is_none());
    let row = fixture.claims.conflict(&conflict.id).await.unwrap();
    assert!(row.resolved_at.is_some());
}

#[tokio::test]
async fn a_claim_collision_without_an_observed_hash_still_renders_the_file() {
    let path = "notes.txt";
    let fixture = Fixture::new(path).await;
    let base_hash = fixture.hash(BASE);

    // `acquire` records a collision with no observed hash: nobody wrote, two agents wanted the
    // file. The three versions are still the file and the claim, not an empty pane.
    let conflict = fixture
        .claims
        .record_conflict(
            path,
            Some(&fixture.claimant),
            Some(&fixture.writer),
            Some(base_hash),
            None,
        )
        .await
        .unwrap();

    let sides = fixture.claims.sides(&conflict.id).await.unwrap();
    assert_eq!(
        sides.theirs.as_ref().map(|side| side.text.clone()),
        Some(BASE.to_string())
    );
    assert_eq!(
        sides.base.as_ref().map(|side| side.text.clone()),
        Some(BASE.to_string())
    );
    assert_eq!(
        sides.ours.as_ref().map(|side| side.text.clone()),
        Some(BASE.to_string())
    );
    assert!(
        sides
            .note
            .unwrap_or_default()
            .contains("no observed version"),
        "the note explains why the row is not a lost write"
    );
}

#[tokio::test]
async fn every_conflicted_hunk_needs_a_choice_and_the_verdict_names_the_reviewer() {
    let path = "notes.txt";
    let fixture = Fixture::new(path).await;
    let base_hash = fixture.hash(BASE);
    let ours_text = BASE
        .replace("two\n", "TWO from ours\n")
        .replace("four\n", "FOUR from ours\n");
    let theirs_text = BASE
        .replace("two\n", "TWO from theirs\n")
        .replace("four\n", "FOUR from theirs\n");
    fixture.write_as(&fixture.claimant, path, &ours_text).await;
    let theirs_hash = fixture.write_as(&fixture.writer, path, &theirs_text).await;

    let conflict = fixture
        .claims
        .record_conflict(
            path,
            Some(&fixture.claimant),
            Some(&fixture.writer),
            Some(base_hash),
            Some(theirs_hash),
        )
        .await
        .unwrap();
    let merged = fixture.claims.merge(&conflict.id).await.unwrap();
    assert_eq!(merged.conflicts, 2, "two lines were edited by both sides");
    assert_eq!(merged.hunks.len(), 2);
    assert_eq!(merged.hunks[0].base, "two");
    assert_eq!(merged.hunks[1].base, "four");

    // A complete decision list cannot smuggle the marker block through.
    let marked = fixture
        .claims
        .resolve_with_merge(
            &fixture.pipeline,
            &fixture.session,
            &conflict.id,
            &merged.merged,
            &fixture.verdict(&merged, "Ada Reviewer"),
        )
        .await;
    assert!(matches!(marked, Err(ClaimError::UnresolvedMarkers)));

    // The reviewer is part of the verdict, and a missing name is refused before any write.
    let mut unnamed = fixture.verdict(&merged, "Ada Reviewer");
    unnamed.reviewer = "   ".to_string();
    let refused = fixture
        .claims
        .resolve_with_merge(
            &fixture.pipeline,
            &fixture.session,
            &conflict.id,
            "one\nTWO from ours\nthree\nFOUR from theirs\nfive\n",
            &unnamed,
        )
        .await;
    assert!(matches!(refused, Err(ClaimError::ReviewerMissing)));

    // Every hunk needs its own decision: one short is not a resolution.
    let mut short = fixture.verdict(&merged, "Ada Reviewer");
    short.decisions.pop();
    let refused = fixture
        .claims
        .resolve_with_merge(
            &fixture.pipeline,
            &fixture.session,
            &conflict.id,
            "one\nTWO from ours\nthree\nFOUR from theirs\nfive\n",
            &short,
        )
        .await;
    assert!(matches!(
        refused,
        Err(ClaimError::VerdictCount {
            expected: 2,
            given: 1
        })
    ));

    let mut verdict = fixture.verdict(&merged, "Ada Reviewer");
    verdict.decisions[0].choice = "ours".to_string();
    verdict.decisions[1].choice = "theirs".to_string();
    verdict.test_hint = "cargo test -p rigel-core --test merge".to_string();
    let resolved = "one\nTWO from ours\nthree\nFOUR from theirs\nfive\n";
    let resolution = fixture
        .claims
        .resolve_with_merge(
            &fixture.pipeline,
            &fixture.session,
            &conflict.id,
            resolved,
            &verdict,
        )
        .await
        .unwrap();
    assert_eq!(resolution.verdict.reviewer, "Ada Reviewer");
    assert_eq!(resolution.verdict.decisions.len(), 2);
    assert_eq!(resolution.verdict.decisions[1].choice, "theirs");
    assert_eq!(resolution.verdict.hunks, 2);
    assert_eq!(fixture.read(path), resolved);

    let stored = fixture.claims.verdict(&conflict.id).await.unwrap().unwrap();
    assert_eq!(stored.reviewer, "Ada Reviewer");
    assert_eq!(stored.decisions.len(), 2);
    assert!(stored.tests_acknowledged);
}
