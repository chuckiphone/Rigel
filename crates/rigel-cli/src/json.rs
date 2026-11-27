//! Versioned JSON output for every `--json` CLI payload (roadmap 141).
//!
//! Every object includes `schema_version`; arrays and scalars are carried in `data`.

use serde::Serialize;
use serde_json::Value;

/// Bump when a breaking field change is made to any `--json` payload.
pub const SCHEMA_VERSION: u32 = 1;

/// Add the schema version to a JSON object or wrap non-object values in an envelope.
pub fn envelope(value: impl Serialize) -> Result<Value, serde_json::Error> {
    let mut value = serde_json::to_value(value)?;
    match &mut value {
        Value::Object(map) => {
            map.insert(
                "schema_version".to_string(),
                Value::Number(SCHEMA_VERSION.into()),
            );
        }
        _ => {
            value = serde_json::json!({
                "schema_version": SCHEMA_VERSION,
                "data": value,
            });
        }
    }
    Ok(value)
}

/// Serialize a payload with its schema version.
pub fn to_string(value: impl Serialize) -> Result<String, serde_json::Error> {
    serde_json::to_string(&envelope(value)?)
}

/// Serialize a payload with its schema version in a readable format.
pub fn to_string_pretty(value: impl Serialize) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&envelope(value)?)
}

/// Print a schema-versioned compact JSON payload. Serialization failures are sent to stderr.
pub fn print(value: impl Serialize) {
    match to_string(value) {
        Ok(text) => println!("{text}"),
        Err(error) => eprintln!("rigel: could not encode JSON: {error}"),
    }
}

/// Print a schema-versioned pretty JSON payload. Serialization failures are sent to stderr.
pub fn print_pretty(value: impl Serialize) {
    match to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(error) => eprintln!("rigel: could not encode JSON: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn objects_keep_fields_and_add_schema_version() {
        let value = envelope(json!({"ok": true, "result": [1]})).unwrap();
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["ok"], true);
        assert_eq!(value["result"], json!([1]));
    }

    #[test]
    fn arrays_and_scalars_are_wrapped() {
        for payload in [json!([1, 2]), json!("text"), json!(null)] {
            let value = envelope(payload.clone()).unwrap();
            assert_eq!(value["schema_version"], SCHEMA_VERSION);
            assert_eq!(value["data"], payload);
        }
    }

    #[test]
    fn serialized_output_has_the_version() {
        let parsed: Value =
            serde_json::from_str(&to_string_pretty(json!({"ok": true})).unwrap()).unwrap();
        assert_eq!(parsed["schema_version"], SCHEMA_VERSION);
    }

    #[test]
    fn breaking_shape_changes_require_a_version_snapshot_update() {
        // Intentional contract snapshot: a breaking output-field change must update this version.
        assert_eq!(
            SCHEMA_VERSION, 1,
            "bump the JSON schema version for breaking shape changes"
        );
    }

    fn regex_like_json_print(source: &str) -> bool {
        let compact = source.split_whitespace().collect::<String>();
        compact.contains("println!(\"{}\",json!(")
            || compact.contains("println!(\"{}\",serde_json::json!(")
            || compact.contains("println!(\"{}\",serde_json::to_string(")
            || compact.contains("println!(\"{}\",serde_json::to_string_pretty(")
    }

    #[test]
    fn cli_sources_route_json_output_through_the_envelope() {
        let sources = [
            include_str!("audit.rs"),
            include_str!("backup.rs"),
            include_str!("batch.rs"),
            include_str!("cloud.rs"),
            include_str!("completions.rs"),
            include_str!("db.rs"),
            include_str!("doctor/mod.rs"),
            include_str!("handoff.rs"),
            include_str!("harness.rs"),
            include_str!("hooks.rs"),
            include_str!("impact.rs"),
            include_str!("local.rs"),
            include_str!("mail.rs"),
            include_str!("main.rs"),
            include_str!("network.rs"),
            include_str!("plugins.rs"),
            include_str!("proof.rs"),
            include_str!("retention.rs"),
            include_str!("route.rs"),
            include_str!("rules.rs"),
            include_str!("serve.rs"),
            include_str!("stats.rs"),
            include_str!("testing.rs"),
            include_str!("tools.rs"),
            include_str!("update.rs"),
            include_str!("worktree.rs"),
            include_str!("phase3/acp.rs"),
            include_str!("phase3/agent.rs"),
            include_str!("phase3/ops.rs"),
            include_str!("phase3/snapshot.rs"),
            include_str!("phase3/trust.rs"),
        ];
        for source in sources {
            let compact = regex_like_json_print(source);
            assert!(
                !compact,
                "raw JSON println remains outside the versioned serializer"
            );
        }
    }
}
