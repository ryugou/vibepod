use vibepod::bridge::slack::{build_idle_notification_blocks, map_action_to_stdin, map_reaction_to_stdin};

#[test]
fn test_block_kit_message_structure() {
    let blocks = build_idle_notification_blocks(
        "Do you want to proceed? (y/n)",
        "my-project",
        "20260325-143000-a1b2",
    );

    // blocks は JSON Value の配列
    assert!(blocks.is_array());
    let blocks = blocks.as_array().unwrap();

    // 最低2ブロック: section (コードブロック) + actions (ボタン)
    assert!(blocks.len() >= 2, "expected at least 2 blocks, got {}", blocks.len());

    // section block が存在し mrkdwn タイプであること
    let section = blocks.iter().find(|b| b["type"] == "section").expect("section block not found");
    assert_eq!(section["text"]["type"], "mrkdwn");
    // コードブロック（バッファ内容）を含むこと
    let text = section["text"]["text"].as_str().unwrap();
    assert!(text.contains("Do you want to proceed? (y/n)"), "section should contain buffer content");

    // actions block が存在し Yes/No/Skip ボタンを含むこと
    let actions = blocks.iter().find(|b| b["type"] == "actions").expect("actions block not found");
    let elements = actions["elements"].as_array().expect("actions should have elements");
    assert_eq!(elements.len(), 3, "expected 3 buttons (Yes, No, Skip)");

    // ボタンの action_id を検証
    let action_ids: Vec<&str> = elements.iter().map(|e| e["action_id"].as_str().unwrap()).collect();
    assert!(action_ids.contains(&"respond_yes"));
    assert!(action_ids.contains(&"respond_no"));
    assert!(action_ids.contains(&"respond_skip"));
}

#[test]
fn test_block_kit_contains_project_and_session() {
    let blocks = build_idle_notification_blocks(
        "output text",
        "my-project",
        "20260325-143000-a1b2",
    );

    let blocks_str = serde_json::to_string(&blocks).unwrap();
    assert!(blocks_str.contains("my-project"), "blocks should contain project name");
    assert!(blocks_str.contains("20260325-143000-a1b2"), "blocks should contain session id");
}

#[test]
fn test_response_mapping_yes() {
    assert_eq!(map_action_to_stdin("respond_yes"), Some("y\n".to_string()));
}

#[test]
fn test_response_mapping_no() {
    assert_eq!(map_action_to_stdin("respond_no"), Some("n\n".to_string()));
}

#[test]
fn test_response_mapping_skip() {
    assert_eq!(map_action_to_stdin("respond_skip"), Some("\n".to_string()));
}

#[test]
fn test_response_mapping_unknown() {
    assert_eq!(map_action_to_stdin("unknown_action"), None);
}

#[test]
fn test_reaction_mapping_thumbsup() {
    assert_eq!(map_reaction_to_stdin("+1"), Some("y\n".to_string()));
    assert_eq!(map_reaction_to_stdin("thumbsup"), Some("y\n".to_string()));
}

#[test]
fn test_reaction_mapping_thumbsdown() {
    assert_eq!(map_reaction_to_stdin("-1"), Some("n\n".to_string()));
    assert_eq!(map_reaction_to_stdin("thumbsdown"), Some("n\n".to_string()));
}

#[test]
fn test_reaction_mapping_skip_emoji() {
    assert_eq!(map_reaction_to_stdin("fast_forward"), Some("\n".to_string()));
    assert_eq!(map_reaction_to_stdin("black_right_pointing_double_triangle_with_vertical_bar"), Some("\n".to_string()));
}

#[test]
fn test_reaction_mapping_unknown() {
    assert_eq!(map_reaction_to_stdin("heart"), None);
}
