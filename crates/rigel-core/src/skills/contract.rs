//! Skill script contract (roadmap 152).
//!
//! A skill that runs a script declares its command-line arguments, its environment
//! variables and the exit codes it may produce. `Contract` is that declaration, and
//! it is checked before a script starts and after it finishes: a script whose
//! invocation does not match the contract is refused rather than run, and a `--dry-run`
//! prints the exact command the contract implies.

use std::collections::BTreeMap;
use std::fmt;

use crate::skills::run::Script;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgSpec {
    pub name: String,
    pub required: bool,
    pub default: Option<String>,
}

impl ArgSpec {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            required: true,
            default: None,
        }
    }

    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    pub fn with_default(mut self, value: impl Into<String>) -> Self {
        self.default = Some(value.into());
        self.required = false;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvSpec {
    pub name: String,
    pub required: bool,
}

impl EnvSpec {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            required: true,
        }
    }

    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contract {
    pub args: Vec<ArgSpec>,
    pub env: Vec<EnvSpec>,
    pub exit_codes: Vec<i32>,
}

impl Contract {
    pub fn new() -> Self {
        Self {
            args: Vec::new(),
            env: Vec::new(),
            exit_codes: vec![0],
        }
    }

    pub fn arg(mut self, spec: ArgSpec) -> Self {
        self.args.push(spec);
        self
    }

    pub fn env(mut self, spec: EnvSpec) -> Self {
        self.env.push(spec);
        self
    }

    pub fn exit_code(mut self, code: i32) -> Self {
        if !self.exit_codes.contains(&code) {
            self.exit_codes.push(code);
        }
        self
    }

    /// The exact command the contract implies for a script, for `--dry-run`.
    pub fn dry_run(&self, script: &Script) -> String {
        script.command_line()
    }

    /// Alias kept for callers that expect a `dry_run_command` entry point.
    pub fn dry_run_command(&self, script: &Script) -> String {
        self.dry_run(script)
    }

    /// Validate that a script's declared arguments and environment match the contract.
    /// A mismatch returns an error and the script must not run.
    pub fn validate_script(&self, script: &Script) -> Result<(), ContractError> {
        // Argument count: required args must be present.
        let required_args = self.args.iter().filter(|a| a.required).count();
        if script.args.len() < required_args {
            return Err(ContractError::MissingArgument {
                expected: required_args,
                got: script.args.len(),
            });
        }
        // Environment: every required env var must be declared in the script's env.
        for spec in self.env.iter().filter(|e| e.required) {
            if !script.env.contains_key(&spec.name) {
                return Err(ContractError::MissingEnvironment(spec.name.clone()));
            }
        }
        // Extra env vars in the script that the contract does not declare are refused.
        for name in script.env.keys() {
            if !self.env.iter().any(|spec| spec.name == *name) {
                return Err(ContractError::UndeclaredEnvironment(name.clone()));
            }
        }
        Ok(())
    }

    /// Alias kept for callers that expect `check` / `matches`.
    pub fn check(&self, script: &Script) -> Result<(), ContractError> {
        self.validate_script(script)
    }

    pub fn matches(&self, script: &Script) -> bool {
        self.validate_script(script).is_ok()
    }

    /// Check an exit code against the contract's allowed set.
    pub fn check_exit(&self, code: Option<i32>) -> Result<(), ContractError> {
        match code {
            Some(code) if self.exit_codes.contains(&code) => Ok(()),
            Some(code) => Err(ContractError::UnexpectedExitCode {
                code,
                allowed: self.exit_codes.clone(),
            }),
            None => Err(ContractError::NoExitCode),
        }
    }
}

impl Default for Contract {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractError {
    MissingArgument { expected: usize, got: usize },
    MissingEnvironment(String),
    UndeclaredEnvironment(String),
    UnexpectedExitCode { code: i32, allowed: Vec<i32> },
    NoExitCode,
}

impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingArgument { expected, got } => {
                write!(f, "missing argument: expected {expected}, got {got}")
            }
            Self::MissingEnvironment(name) => {
                write!(f, "missing required environment variable: {name}")
            }
            Self::UndeclaredEnvironment(name) => {
                write!(f, "undeclared environment variable: {name}")
            }
            Self::UnexpectedExitCode { code, allowed } => {
                write!(f, "unexpected exit code {code} (allowed: {allowed:?})")
            }
            Self::NoExitCode => write!(f, "no exit code (timed out or killed)"),
        }
    }
}

impl std::error::Error for ContractError {}

/// Alias for callers that prefer the `ScriptContract` name.
pub type ScriptContract = Contract;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::run::Script;
    use std::collections::BTreeMap;

    fn script() -> Script {
        let mut env = BTreeMap::new();
        env.insert("API_KEY".to_string(), "secret".to_string());
        Script {
            program: "python".to_string(),
            args: vec!["run.py".to_string()],
            env,
            cwd: std::path::PathBuf::from("."),
            timeout: std::time::Duration::from_secs(60),
            max_output: 1024,
        }
    }

    #[test]
    fn dry_run_prints_the_exact_command() {
        let contract = Contract::new();
        let script = Script::new(".", "python").arg("run.py").arg("--check");
        assert_eq!(contract.dry_run(&script), "python run.py --check");
        assert_eq!(contract.dry_run_command(&script), "python run.py --check");
    }

    #[test]
    fn a_matching_contract_is_accepted() {
        let contract = Contract::new()
            .arg(ArgSpec::new("script"))
            .env(EnvSpec::new("API_KEY"));
        assert!(contract.matches(&script()));
        assert!(contract.check(&script()).is_ok());
    }

    #[test]
    fn a_missing_required_argument_is_refused() {
        let contract = Contract::new()
            .arg(ArgSpec::new("script"))
            .arg(ArgSpec::new("target"));
        let err = contract.validate_script(&script()).unwrap_err();
        assert!(matches!(err, ContractError::MissingArgument { .. }));
    }

    #[test]
    fn a_missing_required_environment_variable_is_refused() {
        let contract = Contract::new().env(EnvSpec::new("API_KEY"));
        let mut s = script();
        s.env.clear();
        let err = contract.validate_script(&s).unwrap_err();
        assert!(matches!(err, ContractError::MissingEnvironment(_)));
    }

    #[test]
    fn an_undeclared_environment_variable_is_refused() {
        let contract = Contract::new(); // declares nothing
        let err = contract.validate_script(&script()).unwrap_err();
        assert!(matches!(err, ContractError::UndeclaredEnvironment(_)));
    }

    #[test]
    fn an_unexpected_exit_code_is_refused() {
        let contract = Contract::new().exit_code(0).exit_code(2);
        assert!(contract.check_exit(Some(0)).is_ok());
        assert!(contract.check_exit(Some(2)).is_ok());
        let err = contract.check_exit(Some(1)).unwrap_err();
        assert!(matches!(err, ContractError::UnexpectedExitCode { .. }));
        assert!(contract.check_exit(None).is_err());
    }

    #[test]
    fn optional_args_and_env_do_not_block() {
        let contract = Contract::new()
            .arg(ArgSpec::new("script").optional())
            .env(EnvSpec::new("API_KEY").optional());
        // No args, no env: still ok because both optional.
        let s = Script::new(".", "python");
        assert!(contract.matches(&s));
    }
}
