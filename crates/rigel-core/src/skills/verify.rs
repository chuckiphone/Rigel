//! Roadmap 77: a skill's instructions are only loaded when they are the bytes a trusted
//! publisher signed, unless the project explicitly accepts unsigned skills.
//!
//! A signed `SKILL.md` carries three frontmatter fields:
//!
//! ```text
//! publisher: acme
//! content-sha256: <sha256 of the body after the frontmatter, lowercase hex>
//! signature: <ed25519 signature over the payload below, lowercase hex>
//! ```
//!
//! The signed payload is:
//!
//! ```text
//! rigel-skill-v1\n<id>\n<publisher>\n<content-sha256>\n
//! ```
//!
//! Binding the skill id stops a signature being moved to another row. The public key lives in
//! the project's `.rigel/skill-trust.json`; a key shipped inside the same file as the signature
//! proves nothing.
//!
//! Built-in skills are compiled into the binary and indexed as `builtin:<slug>`; their origin is
//! the build, not a downloaded repository, so they do not need a publisher signature. Every
//! other source is refused when unsigned unless the project sets `allow_unsigned: true`.
//!
//! This gate covers the instructions. A skill never grants a tool, and a script it names still
//! needs the separate `run_command` approval.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use super::blob::sha256_hex;
use super::frontmatter;

/// The project policy file, relative to the project root.
pub const TRUST_FILE: &str = ".rigel/skill-trust.json";

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error(
        "skill {0} is unsigned; add its publisher to {TRUST_FILE}, or set \
         \"allow_unsigned\": true there to accept unsigned skills in this project"
    )]
    Unsigned(String),

    #[error("skill {id} is signed by {publisher}, whose public key is not trusted in {TRUST_FILE}")]
    UnknownPublisher { id: String, publisher: String },

    #[error("skill {0} does not match its signed content hash")]
    HashMismatch(String),

    #[error("skill {0} has an invalid publisher signature")]
    InvalidSignature(String),

    #[error("skill {id} has malformed signing metadata: {detail}")]
    Malformed { id: String, detail: String },

    #[error("could not read {path}: {source}")]
    Config {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("{path} is not a valid trust policy: {detail}")]
    ConfigJson { path: PathBuf, detail: String },
}

impl VerifyError {
    /// Stable code for the tool envelope. A model can correct a missing opt-in differently from
    /// a body that changed after signing.
    pub fn code(&self) -> &'static str {
        match self {
            VerifyError::Unsigned(_) => "UNSIGNED_SKILL",
            VerifyError::Config { .. } | VerifyError::ConfigJson { .. } => "SKILL_TRUST_CONFIG",
            VerifyError::UnknownPublisher { .. }
            | VerifyError::HashMismatch(_)
            | VerifyError::InvalidSignature(_)
            | VerifyError::Malformed { .. } => "SKILL_SIGNATURE_INVALID",
        }
    }
}

/// What the load gate concluded. The body is returned only after this succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verification {
    /// Compiled into the binary; there is no downloaded publisher to trust.
    Builtin,
    /// Signed by a publisher whose key is in the project policy.
    Signed { publisher: String },
    /// Unsigned, but the project opted in.
    Unsigned,
}

impl Verification {
    pub fn status(&self) -> &'static str {
        match self {
            Verification::Builtin => "builtin",
            Verification::Signed { .. } => "signed",
            Verification::Unsigned => "unsigned",
        }
    }

    pub fn publisher(&self) -> Option<&str> {
        match self {
            Verification::Signed { publisher } => Some(publisher),
            Verification::Builtin | Verification::Unsigned => None,
        }
    }
}

/// The project's trust policy. A missing file is the safe default: no trusted publishers and no
/// unsigned skills.
#[derive(Debug, Clone, Default)]
pub struct SkillTrust {
    allow_unsigned: bool,
    publishers: BTreeMap<String, VerifyingKey>,
}

#[derive(serde::Deserialize, Default)]
struct TrustFile {
    #[serde(default)]
    allow_unsigned: bool,
    #[serde(default)]
    publishers: BTreeMap<String, String>,
}

impl SkillTrust {
    /// Read `<project>/.rigel/skill-trust.json`. A missing file means the defaults; unreadable or
    /// malformed policy fails closed for every non-built-in skill.
    pub fn load(project_root: &Path) -> Result<Self, VerifyError> {
        let path = project_root.join(TRUST_FILE);
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(VerifyError::Config { path, source }),
        };
        Self::from_json(&path, &raw)
    }

    pub fn from_json(path: &Path, raw: &str) -> Result<Self, VerifyError> {
        let file: TrustFile =
            serde_json::from_str(raw).map_err(|error| VerifyError::ConfigJson {
                path: path.to_path_buf(),
                detail: error.to_string(),
            })?;

        let mut publishers = BTreeMap::new();
        for (name, encoded) in file.publishers {
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err(VerifyError::ConfigJson {
                    path: path.to_path_buf(),
                    detail: "a publisher name is empty".to_string(),
                });
            }
            let bytes = decode_hex(encoded.trim(), 32).ok_or_else(|| VerifyError::ConfigJson {
                path: path.to_path_buf(),
                detail: format!("publisher {name} does not have a 32-byte hex public key"),
            })?;
            let key_bytes: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .expect("decode_hex enforced 32 bytes");
            let key =
                VerifyingKey::from_bytes(&key_bytes).map_err(|error| VerifyError::ConfigJson {
                    path: path.to_path_buf(),
                    detail: format!("publisher {name} has an invalid ed25519 key: {error}"),
                })?;
            publishers.insert(name, key);
        }

        Ok(Self {
            allow_unsigned: file.allow_unsigned,
            publishers,
        })
    }

    pub fn allow_unsigned(&self) -> bool {
        self.allow_unsigned
    }

    pub fn publishers(&self) -> impl Iterator<Item = &str> {
        self.publishers.keys().map(String::as_str)
    }

    /// Decide whether these exact bytes may be shown to an agent as a loaded skill.
    pub fn verify(&self, id: &str, source: &str, body: &str) -> Result<Verification, VerifyError> {
        if source == "builtin" && id.starts_with("builtin:") {
            return Ok(Verification::Builtin);
        }

        let parsed = frontmatter::parse(body);
        let publisher = parsed
            .get("publisher")
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let content_sha = parsed
            .get("content-sha256")
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let signature = parsed
            .get("signature")
            .map(str::trim)
            .filter(|s| !s.is_empty());

        match (publisher, content_sha, signature) {
            (None, None, None) => {
                if self.allow_unsigned {
                    Ok(Verification::Unsigned)
                } else {
                    Err(VerifyError::Unsigned(id.to_string()))
                }
            }
            (Some(publisher), Some(content_sha), Some(signature)) => {
                self.verify_signed(id, publisher, content_sha, signature, &parsed.body)
            }
            _ => Err(VerifyError::Malformed {
                id: id.to_string(),
                detail:
                    "publisher, content-sha256 and signature must all be present or all be absent"
                        .to_string(),
            }),
        }
    }

    fn verify_signed(
        &self,
        id: &str,
        publisher: &str,
        content_sha: &str,
        signature: &str,
        body: &str,
    ) -> Result<Verification, VerifyError> {
        let Some(key) = self.publishers.get(publisher) else {
            return Err(VerifyError::UnknownPublisher {
                id: id.to_string(),
                publisher: publisher.to_string(),
            });
        };
        let signed_bytes = decode_hex(content_sha, 32).ok_or_else(|| VerifyError::Malformed {
            id: id.to_string(),
            detail: "content-sha256 is not a 32-byte hex digest".to_string(),
        })?;
        let signature_bytes = decode_hex(signature, 64).ok_or_else(|| VerifyError::Malformed {
            id: id.to_string(),
            detail: "signature is not a 64-byte hex value".to_string(),
        })?;
        let signature =
            Signature::from_slice(&signature_bytes).map_err(|_| VerifyError::Malformed {
                id: id.to_string(),
                detail: "signature is not a valid ed25519 signature".to_string(),
            })?;

        let payload = signing_payload(id, publisher, content_sha);
        key.verify(payload.as_bytes(), &signature)
            .map_err(|_| VerifyError::InvalidSignature(id.to_string()))?;

        let actual = sha256_hex(body.as_bytes());
        if actual != hex(&signed_bytes) {
            return Err(VerifyError::HashMismatch(id.to_string()));
        }

        Ok(Verification::Signed {
            publisher: publisher.to_string(),
        })
    }
}

/// The exact bytes a publisher signs. Public so a signing tool and a test can agree without
/// reproducing the format by hand.
pub fn signing_payload(id: &str, publisher: &str, content_sha: &str) -> String {
    format!("rigel-skill-v1\n{id}\n{publisher}\n{content_sha}\n")
}

/// SHA-256 of the instructions after the frontmatter. Metadata is not signed; it is not loaded
/// as instructions, and the body is what the agent receives.
pub fn content_sha256(body: &str) -> String {
    sha256_hex(frontmatter::parse(body).body.as_bytes())
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16).expect("nibble"));
        out.push(char::from_digit((byte & 0x0f) as u32, 16).expect("nibble"));
    }
    out
}

fn decode_hex(text: &str, expected: usize) -> Option<Vec<u8>> {
    if text.len() != expected * 2 {
        return None;
    }
    let mut bytes = Vec::with_capacity(expected);
    let raw = text.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        let high = (raw[index] as char).to_digit(16)?;
        let low = (raw[index + 1] as char).to_digit(16)?;
        bytes.push(((high << 4) | low) as u8);
        index += 2;
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    const ID: &str = "github:acme/skills:setup/SKILL.md";

    fn keys() -> (SigningKey, String) {
        let secret = SigningKey::from_bytes(&[7_u8; 32]);
        let public = hex(secret.verifying_key().as_bytes());
        (secret, public)
    }

    fn signed(id: &str, publisher: &str, content: &str) -> String {
        let (secret, _) = keys();
        let base = format!("---\nname: setup\ndescription: Set the project up.\n---\n{content}");
        let sha = content_sha256(&base);
        let payload = signing_payload(id, publisher, &sha);
        let signature = hex(&secret.sign(payload.as_bytes()).to_bytes());
        format!(
            "---\nname: setup\ndescription: Set the project up.\npublisher: {publisher}\n\
             content-sha256: {sha}\nsignature: {signature}\n---\n{content}"
        )
    }

    fn trust(public: &str, allow_unsigned: bool) -> SkillTrust {
        let raw = format!(
            "{{\"allow_unsigned\":{allow_unsigned},\"publishers\":{{\"acme\":\"{public}\"}}}}"
        );
        SkillTrust::from_json(Path::new("test.json"), &raw).unwrap()
    }

    #[test]
    fn a_matching_signature_and_hash_are_accepted() {
        let (_, public) = keys();
        let body = signed(ID, "acme", "# Setup\n\nRun the setup.\n");
        let verdict = trust(&public, false).verify(ID, "github", &body).unwrap();
        assert_eq!(
            verdict,
            Verification::Signed {
                publisher: "acme".to_string()
            }
        );
    }

    #[test]
    fn a_tampered_body_is_refused_even_with_a_real_signature() {
        let (_, public) = keys();
        let signed_body = signed(ID, "acme", "# Setup\n\nRun the setup.\n");
        let tampered = signed_body.replace("Run the setup.", "Delete everything.");
        let error = trust(&public, false)
            .verify(ID, "github", &tampered)
            .unwrap_err();
        assert!(matches!(error, VerifyError::HashMismatch(_)), "{error}");
    }

    #[test]
    fn a_signature_for_another_skill_id_is_refused() {
        let (_, public) = keys();
        let body = signed(ID, "acme", "# Setup\n\nRun the setup.\n");
        let error = trust(&public, false)
            .verify("github:acme/skills:other/SKILL.md", "github", &body)
            .unwrap_err();
        assert!(matches!(error, VerifyError::InvalidSignature(_)), "{error}");
    }

    #[test]
    fn an_unknown_publisher_is_refused() {
        let (_, public) = keys();
        let body = signed(ID, "mallory", "# Setup\n\nRun the setup.\n");
        let error = trust(&public, false)
            .verify(ID, "github", &body)
            .unwrap_err();
        assert!(
            matches!(error, VerifyError::UnknownPublisher { ref publisher, .. } if publisher == "mallory"),
            "{error}"
        );
    }

    #[test]
    fn an_unsigned_body_needs_the_explicit_opt_in() {
        let (_, public) = keys();
        let body = "---\nname: setup\n---\n# Setup\n";
        let error = trust(&public, false)
            .verify(ID, "github", body)
            .unwrap_err();
        assert!(matches!(error, VerifyError::Unsigned(_)), "{error}");
        assert_eq!(
            trust(&public, true).verify(ID, "github", body).unwrap(),
            Verification::Unsigned
        );
    }

    #[test]
    fn builtins_are_trusted_without_a_signature() {
        let body = "---\nname: setup\n---\n# Setup\n";
        assert_eq!(
            SkillTrust::default()
                .verify("builtin:setup", "builtin", body)
                .unwrap(),
            Verification::Builtin
        );
        // A downloaded row cannot claim built-in provenance by giving itself that id.
        assert!(
            SkillTrust::default()
                .verify("builtin:setup", "github", body)
                .is_err()
        );
    }

    #[test]
    fn a_partial_signature_block_is_malformed_not_unsigned() {
        let body = "---\nname: setup\npublisher: acme\n---\n# Setup\n";
        let error = SkillTrust::default()
            .verify(ID, "github", body)
            .unwrap_err();
        assert!(matches!(error, VerifyError::Malformed { .. }), "{error}");
        assert_eq!(error.code(), "SKILL_SIGNATURE_INVALID");
    }

    #[test]
    fn a_malformed_policy_fails_closed() {
        let error = SkillTrust::from_json(
            Path::new("test.json"),
            "{\"publishers\":{\"acme\":\"not-hex\"}}",
        )
        .unwrap_err();
        assert!(matches!(error, VerifyError::ConfigJson { .. }), "{error}");
        assert_eq!(error.code(), "SKILL_TRUST_CONFIG");
    }
}
