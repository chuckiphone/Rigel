//! Skill script contract (roadmap 152) integration tests.

use rigel_core::skills::contract::{Contract, ContractError};
use rigel_core::skills::run::Script;

#[test]
fn dry_run_prints_exact_command_and_refusal_works() {
    let contract = Contract::new().arg(rigel_core::skills::contract::ArgSpec::new("script"));
    let script = Script::new(".", "python").arg("run.py");
    assert_eq!(contract.dry_run(&script), "python run.py");
    assert!(contract.matches(&script));

    let bad = Script::new(".", "python"); // missing arg
    assert!(matches!(
        contract.check(&bad),
        Err(ContractError::MissingArgument { .. })
    ));
    let extra = Script::new(".", "python").arg("run.py").arg("extra");
    assert!(matches!(
        contract.check(&extra),
        Err(ContractError::UnexpectedArgumentCount { .. })
    ));
}

#[test]
fn exit_code_contract_is_enforced() {
    let contract = Contract::new().exit_code(0);
    assert!(contract.check_exit(Some(0)).is_ok());
    assert!(contract.check_exit(Some(1)).is_err());
}

#[test]
fn skill_frontmatter_loads_the_script_and_refuses_contract_mismatches() {
    let source = "---\nscript-program: python\nscript-args: file ?mode\nscript-env: ?TOKEN\nscript-exit-codes: 0, 3\n---\n";
    let (contract, script) = Contract::load_skill(".", source, vec!["run.py".into()]).unwrap();
    assert_eq!(contract.dry_run(&script), "python run.py");
    assert!(contract.check(&script).is_ok());

    let error = Contract::load_skill(".", source, Vec::new()).unwrap_err();
    assert!(matches!(error, ContractError::MissingArgument { .. }));
}

#[test]
fn declared_default_arguments_are_included_in_the_run_command() {
    let source = "---\nscript-program: python\nscript-args: file=default.py ?mode\n---\n";
    let (contract, script) = Contract::load_skill(".", source, Vec::new()).unwrap();
    assert_eq!(contract.dry_run(&script), "python default.py");
}

#[test]
fn missing_required_script_environment_is_refused_before_execution() {
    let name = format!("RIGEL_MISSING_{}", uuid::Uuid::new_v4().simple());
    let source =
        format!("---\nscript-program: python\nscript-args: file\nscript-env: {name}\n---\n");
    let error = Contract::load_skill(".", &source, vec!["run.py".into()]).unwrap_err();
    assert_eq!(error, ContractError::MissingEnvironment(name));
}

#[test]
fn checked_runner_refuses_a_mismatch_before_attempting_to_spawn() {
    let contract = Contract::new().arg(rigel_core::skills::contract::ArgSpec::new("script"));
    let script = Script::new(".", "this-program-must-never-be-spawned");
    let error = rigel_core::skills::run::run_checked(
        std::path::Path::new("."),
        &script,
        &contract,
        &std::collections::BTreeMap::new(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        rigel_core::skills::run::RunError::Contract(ContractError::MissingArgument { .. })
    ));
}
