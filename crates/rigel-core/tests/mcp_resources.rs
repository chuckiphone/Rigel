//! MCP resources and prompts (roadmap 146) integration tests.

use rigel_core::mcp::ServerSpec;
use rigel_core::mcp::resources::*;
use std::time::Duration;

#[tokio::test]
async fn an_empty_command_fails_cleanly() {
    let server = ServerSpec {
        name: "empty".to_string(),
        command: String::new(),
        args: vec![],
        env: vec![],
    };
    assert!(
        list_resources(&server, Duration::from_secs(1))
            .await
            .is_err()
    );
    assert!(list_prompts(&server, Duration::from_secs(1)).await.is_err());
}

#[tokio::test]
async fn a_fake_server_discovers_resources_and_prompts() {
    let Ok(path) = std::env::var("RIGEL_FAKE_MCP_SERVER") else {
        return;
    };
    let server = ServerSpec {
        name: "demo".to_string(),
        command: path,
        args: vec![],
        env: vec![],
    };
    let resources = list_resources(&server, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].uri, "file:///a.txt");

    let read = read_resource(&server, "file:///a.txt", Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(read.contents[0].text.as_deref(), Some("hello"));

    let prompts = list_prompts(&server, Duration::from_secs(5)).await.unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].name, "greet");

    let got = get_prompt(&server, "greet", None, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(got.messages.len(), 1);
}
