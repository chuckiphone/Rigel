//! Roadmap 103: one note per bookmark.
//!
//! A bookmark is a reader's mark on a transcript line or a diff hunk. Its label is the one-line
//! snapshot the list draws; the note is an optional, equally bounded line of context kept beside
//! it. Both are plain text: the list hands them to the renderer as text, and the session export
//! carries them through the shared redactor. Old stars have no note and are not backfilled.

use serde::Serialize;
use uuid::Uuid;

use super::{SessionError, SessionService};
use crate::migrate::now_ms;

/** A note is one line and this many characters; longer input is clamped, never rejected. */
pub const NOTE_LIMIT: usize = 500;
type BookmarkRow = (String, String, String, String, String, Option<String>, i64);

#[derive(Debug, Clone, Serialize)]
pub struct Bookmark {
    pub id: String,
    pub session_id: String,
    pub kind: String,
    pub target_id: String,
    pub label: String,
    /// Roadmap 103: `None` means an ordinary star, including every row written before migration 26.
    pub note: Option<String>,
    pub created_at: i64,
}

/** One trimmed line or nothing; blank notes are stored as NULL rather than empty strings. */
pub fn normalize_note(note: &str) -> Option<String> {
    let line = note.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return None;
    }
    Some(line.chars().take(NOTE_LIMIT).collect())
}

/** The Markdown rows in the same order `SessionService::bookmarks` returns them. */
pub fn bookmark_lines(rows: &[Bookmark]) -> String {
    if rows.is_empty() {
        return "No bookmarks in this session.\n".to_string();
    }
    let mut out = String::new();
    for row in rows {
        out.push_str(&format!(
            "- {} · `{}` · {}{}\n",
            row.kind,
            row.target_id,
            row.label,
            row.note
                .as_deref()
                .map(|note| format!(" — note: {note}"))
                .unwrap_or_default(),
        ));
    }
    out
}

impl SessionService {
    /// D150 / roadmap 103: newest first, with `id` as the deterministic tie-break. A star is a
    /// note to the reader, so the list is read like one and never reordered between reads.
    pub async fn bookmarks(&self, session_id: &str) -> Result<Vec<Bookmark>, SessionError> {
        let rows: Vec<BookmarkRow> = sqlx::query_as(
            "SELECT id, session_id, kind, target_id, label, note, created_at
                 FROM bookmarks WHERE session_id = ? ORDER BY created_at DESC, id DESC",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(id, session_id, kind, target_id, label, note, created_at)| Bookmark {
                    id,
                    session_id,
                    kind,
                    target_id,
                    label,
                    note,
                    created_at,
                },
            )
            .collect())
    }

    /// D150: starring twice clears the star. Kept for the callers that have no note to store.
    pub async fn toggle_bookmark(
        &self,
        session_id: &str,
        kind: &str,
        target_id: &str,
        label: &str,
    ) -> Result<Option<Bookmark>, SessionError> {
        self.toggle_bookmark_with_note(session_id, kind, target_id, label, None)
            .await
    }

    /// Roadmap 103: the same toggle with one optional note. A note is normalized before it is
    /// written, so a pasted paragraph cannot turn the list into a wall of text.
    pub async fn toggle_bookmark_with_note(
        &self,
        session_id: &str,
        kind: &str,
        target_id: &str,
        label: &str,
        note: Option<&str>,
    ) -> Result<Option<Bookmark>, SessionError> {
        if !matches!(kind, "item" | "hunk") {
            return Err(SessionError::InvalidBookmarkKind(kind.to_string()));
        }
        // A star on a missing session would fail the foreign key with a database error; name
        // the real problem instead.
        self.get(session_id).await?;

        let existing: Option<String> = sqlx::query_scalar(
            "SELECT id FROM bookmarks WHERE session_id = ? AND kind = ? AND target_id = ?",
        )
        .bind(session_id)
        .bind(kind)
        .bind(target_id)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(id) = existing {
            sqlx::query("DELETE FROM bookmarks WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
                .await?;
            return Ok(None);
        }

        let label = label
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .chars()
            .take(160)
            .collect::<String>();
        let bookmark = Bookmark {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            kind: kind.to_string(),
            target_id: target_id.to_string(),
            label,
            note: note.and_then(normalize_note),
            created_at: now_ms(),
        };
        sqlx::query(
            "INSERT INTO bookmarks (id, session_id, kind, target_id, label, note, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&bookmark.id)
        .bind(&bookmark.session_id)
        .bind(&bookmark.kind)
        .bind(&bookmark.target_id)
        .bind(&bookmark.label)
        .bind(&bookmark.note)
        .bind(bookmark.created_at)
        .execute(&self.pool)
        .await?;

        Ok(Some(bookmark))
    }

    /// Change one bookmark's note without touching its label or target. `None` clears it.
    /// Returns whether a row in this session was updated, so a stale id is never a silent success.
    pub async fn set_bookmark_note(
        &self,
        session_id: &str,
        id: &str,
        note: Option<&str>,
    ) -> Result<bool, SessionError> {
        let changed = sqlx::query("UPDATE bookmarks SET note = ? WHERE id = ? AND session_id = ?")
            .bind(note.and_then(normalize_note))
            .bind(id)
            .bind(session_id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(changed > 0)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn a_note_is_one_trimmed_bounded_line_or_nothing() {
        assert_eq!(normalize_note("  \n  "), None);
        assert_eq!(normalize_note(""), None);
        assert_eq!(
            normalize_note(" check this after the revert \nsecond line"),
            Some("check this after the revert".to_string())
        );
        let long = "x".repeat(NOTE_LIMIT + 20);
        assert_eq!(normalize_note(&long), Some("x".repeat(NOTE_LIMIT)));
    }

    #[test]
    fn the_export_lines_keep_list_order_and_show_only_written_notes() {
        let row = |kind: &str, target: &str, note: Option<&str>| Bookmark {
            id: format!("b-{target}"),
            session_id: "s-1".to_string(),
            kind: kind.to_string(),
            target_id: target.to_string(),
            label: format!("label {target}"),
            note: note.map(str::to_string),
            created_at: 1,
        };
        let lines = bookmark_lines(&[
            row("item", "2", None),
            row("hunk", "1", Some("compare with the patch")),
        ]);
        assert_eq!(
            lines,
            "- item · `2` · label 2\n- hunk · `1` · label 1 — note: compare with the patch\n"
        );
        assert_eq!(bookmark_lines(&[]), "No bookmarks in this session.\n");
    }

    #[tokio::test]
    async fn a_bookmark_note_is_stored_listed_in_order_and_changeable() {
        let tmp = TempDir::new().unwrap();
        let service = SessionService::open(tmp.path()).await.unwrap();
        let session = service.start(Some("notes"), None).await.unwrap();

        let first = service
            .toggle_bookmark_with_note(
                &session.id,
                "item",
                "item-1",
                "turn 1 · read_file",
                Some("  why this read matters\nsecond line "),
            )
            .await
            .unwrap()
            .expect("first toggle is on");
        assert_eq!(first.note.as_deref(), Some("why this read matters"));
        assert!(first.label.contains("read_file"));

        let second = service
            .toggle_bookmark_with_note(&session.id, "hunk", "hunk-1", "patch · @@ -1 +1 @@", None)
            .await
            .unwrap()
            .expect("second toggle is on");
        assert_eq!(second.note, None);

        let rows = service.bookmarks(&session.id).await.unwrap();
        assert_eq!(rows.len(), 2);
        // Newest first: insertion order is the order the session lists.
        assert_eq!(rows[0].id, second.id);
        assert_eq!(rows[1].id, first.id);
        assert_eq!(rows[1].note.as_deref(), Some("why this read matters"));

        assert!(
            service
                .set_bookmark_note(&session.id, &first.id, Some("a better reason"))
                .await
                .unwrap()
        );
        let rows = service.bookmarks(&session.id).await.unwrap();
        assert_eq!(rows[1].note.as_deref(), Some("a better reason"));
        assert!(
            service
                .set_bookmark_note(&session.id, &first.id, None)
                .await
                .unwrap()
        );
        assert_eq!(service.bookmarks(&session.id).await.unwrap()[1].note, None);
        assert!(
            !service
                .set_bookmark_note(&session.id, "missing", Some("x"))
                .await
                .unwrap(),
            "a stale id is not a silent success"
        );

        // Reopening reads the same notes: the column survived the migration and a restart.
        let reopened = SessionService::open(tmp.path()).await.unwrap();
        assert_eq!(reopened.bookmarks(&session.id).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn toggling_a_noted_bookmark_clears_it_with_a_tombstone_free_delete() {
        let tmp = TempDir::new().unwrap();
        let service = SessionService::open(tmp.path()).await.unwrap();
        let session = service.start(None, None).await.unwrap();
        service
            .toggle_bookmark_with_note(&session.id, "item", "item-9", "keep", Some("note"))
            .await
            .unwrap();
        assert!(
            service
                .toggle_bookmark_with_note(&session.id, "item", "item-9", "again", None)
                .await
                .unwrap()
                .is_none()
        );
        assert!(service.bookmarks(&session.id).await.unwrap().is_empty());
    }
}
