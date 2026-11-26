//! Skill script contract (roadmap 152) integration tests.

use rigel_core::skills::contract::{Contract, ContractError};
use rigel_core::skills::run::Script;
use std::collections::BTreeMap;

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
}

#[test]
fn exit_code_contract_is_enforced() {
    let contract = Contract::new().exit_code(0);
    assert!(contract.check_exit(Some(0)).is_ok());
    assert!(contract.check_exit(Some(1)).is_err());
}