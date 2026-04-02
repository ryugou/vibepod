use vibepod::bridge::slack::{
    build_idle_notification_blocks, map_action_to_stdin, map_reaction_to_stdin,
};

#[test]
fn test_block_kit_message_structure() {
    let choices = vec!["yes".to_string(), "no".to_string()];
    let blocks = build_idle_notification_blocks(
        "Do you want to proceed? (y/n)",
        "my-project",
        "20260325-143000-a1b2",
        &choices,
    );

    // blocks は JSON Value の配列
    assert!(blocks.is_array());
    let blocks = blocks.as_array().unwrap();

    // 最低2ブロック: section + actions + context
    assert!(
        blocks.len() >= 2,
        "expected at least 2 blocks, got {}",
        blocks.len()
    );

    // section block が存在し mrkdwn タイプであること
    let section = blocks
        .iter()
        .find(|b| b["type"] == "section")
        .expect("section block not found");
    assert_eq!(section["text"]["type"], "mrkdwn");
    let text = section["text"]["text"].as_str().unwrap();
    assert!(
        text.contains("Do you want to proceed? (y/n)"),
        "section should contain buffer content"
    );

    // actions block が存在し Yes/No/Skip ボタンを含むこと
    let actions = blocks
        .iter()
        .find(|b| b["type"] == "actions")
        .expect("actions block not found");
    let elements = actions["elements"]
        .as_array()
        .expect("actions should have elements");
    assert_eq!(elements.len(), 3, "expected 3 buttons (Yes, No, Skip)");
}

#[test]
fn test_block_kit_contains_project_and_session() {
    let choices: Vec<String> = vec![];
    let blocks = build_idle_notification_blocks(
        "output text",
        "my-project",
        "20260325-143000-a1b2",
        &choices,
    );

    let blocks_str = serde_json::to_string(&blocks).unwrap();
    assert!(
        blocks_str.contains("my-project"),
        "blocks should contain project name"
    );
    assert!(
        blocks_str.contains("20260325-143000-a1b2"),
        "blocks should contain session id"
    );
}

#[test]
fn test_response_mapping_yes_legacy() {
    // レガシー互換: Claude Code は y/n 単文字を期待
    assert_eq!(map_action_to_stdin("respond_yes"), Some("y\r".to_string()));
}

#[test]
fn test_response_mapping_no_legacy() {
    assert_eq!(map_action_to_stdin("respond_no"), Some("n\r".to_string()));
}

#[test]
fn test_response_mapping_skip() {
    assert_eq!(map_action_to_stdin("respond_skip"), Some("\r".to_string()));
}

#[test]
fn test_response_mapping_unknown() {
    assert_eq!(map_action_to_stdin("unknown_action"), None);
}

#[test]
fn test_reaction_mapping_thumbsup() {
    assert_eq!(map_reaction_to_stdin("+1"), Some("y\r".to_string()));
    assert_eq!(map_reaction_to_stdin("thumbsup"), Some("y\r".to_string()));
}

#[test]
fn test_reaction_mapping_thumbsdown() {
    assert_eq!(map_reaction_to_stdin("-1"), Some("n\r".to_string()));
    assert_eq!(map_reaction_to_stdin("thumbsdown"), Some("n\r".to_string()));
}

#[test]
fn test_reaction_mapping_skip_emoji() {
    assert_eq!(
        map_reaction_to_stdin("fast_forward"),
        Some("\r".to_string())
    );
    assert_eq!(
        map_reaction_to_stdin("black_right_pointing_double_triangle_with_vertical_bar"),
        Some("\r".to_string())
    );
}

#[test]
fn test_reaction_mapping_unknown() {
    assert_eq!(map_reaction_to_stdin("heart"), None);
}

// Dynamic button tests

#[test]
fn test_dynamic_buttons_yes_no() {
    let choices = vec!["yes".to_string(), "no".to_string()];
    let blocks = build_idle_notification_blocks("Proceed? (y/n)", "proj", "sess", &choices);
    let blocks = blocks.as_array().unwrap();
    let actions = blocks
        .iter()
        .find(|b| b["type"] == "actions")
        .expect("actions block");
    let elements = actions["elements"].as_array().unwrap();
    // yes, no, Skip の3ボタン
    assert_eq!(elements.len(), 3);
    assert_eq!(elements[0]["action_id"], "respond_choice_yes");
    assert_eq!(elements[0]["text"]["text"], "Yes");
    assert_eq!(elements[1]["action_id"], "respond_choice_no");
    assert_eq!(elements[1]["text"]["text"], "No");
    assert_eq!(elements[2]["action_id"], "respond_skip");
}

#[test]
fn test_dynamic_buttons_abc() {
    let choices = vec!["A".to_string(), "B".to_string(), "C".to_string()];
    let blocks = build_idle_notification_blocks("Choose:", "proj", "sess", &choices);
    let blocks = blocks.as_array().unwrap();
    let actions = blocks
        .iter()
        .find(|b| b["type"] == "actions")
        .expect("actions block");
    let elements = actions["elements"].as_array().unwrap();
    assert_eq!(elements.len(), 4); // A, B, C, Skip
    assert_eq!(elements[0]["action_id"], "respond_choice_A");
    assert_eq!(elements[1]["action_id"], "respond_choice_B");
    assert_eq!(elements[2]["action_id"], "respond_choice_C");
    assert_eq!(elements[3]["action_id"], "respond_skip");
}

#[test]
fn test_dynamic_buttons_no_choices() {
    let choices: Vec<String> = vec![];
    let blocks = build_idle_notification_blocks("Status output", "proj", "sess", &choices);
    let blocks = blocks.as_array().unwrap();
    // ボタンなし — actions block が存在しない
    let actions = blocks.iter().find(|b| b["type"] == "actions");
    assert!(actions.is_none());
}

#[test]
fn test_dynamic_buttons_numbered() {
    let choices = vec!["1".to_string(), "2".to_string(), "3".to_string()];
    let blocks = build_idle_notification_blocks("Pick:", "proj", "sess", &choices);
    let blocks = blocks.as_array().unwrap();
    let actions = blocks
        .iter()
        .find(|b| b["type"] == "actions")
        .expect("actions block");
    let elements = actions["elements"].as_array().unwrap();
    assert_eq!(elements.len(), 4); // 1, 2, 3, Skip
    assert_eq!(elements[0]["action_id"], "respond_choice_1");
    assert_eq!(elements[1]["action_id"], "respond_choice_2");
    assert_eq!(elements[2]["action_id"], "respond_choice_3");
    assert_eq!(elements[3]["action_id"], "respond_skip");
}

#[test]
fn test_map_action_dynamic_choice() {
    assert_eq!(
        map_action_to_stdin("respond_choice_A"),
        Some("A\r".to_string())
    );
    assert_eq!(
        map_action_to_stdin("respond_choice_yes"),
        Some("yes\r".to_string())
    );
    assert_eq!(
        map_action_to_stdin("respond_choice_no"),
        Some("no\r".to_string())
    );
    assert_eq!(
        map_action_to_stdin("respond_choice_1"),
        Some("1\r".to_string())
    );
    assert_eq!(map_action_to_stdin("respond_skip"), Some("\r".to_string()));
}

#[test]
fn test_choice_display_truncation_boundary() {
    // ちょうど20文字 → 切り詰めなし
    let choice_20 = "12345678901234567890".to_string();
    let blocks = build_idle_notification_blocks("Pick:", "proj", "sess", &[choice_20]);
    let elements = blocks
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["type"] == "actions")
        .unwrap()["elements"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(elements[0]["text"]["text"], "12345678901234567890");

    // 21文字 → 20文字 + "..." に切り詰め
    let choice_21 = "123456789012345678901".to_string();
    let blocks = build_idle_notification_blocks("Pick:", "proj", "sess", &[choice_21]);
    let elements = blocks
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["type"] == "actions")
        .unwrap()["elements"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(elements[0]["text"]["text"], "12345678901234567890...");
}

#[test]
fn test_choice_display_truncation_japanese() {
    // 日本語21文字 → 20文字 + "..."
    let choice = "あいうえおかきくけこさしすせそたちつてとな".to_string();
    assert_eq!(choice.chars().count(), 21);
    let blocks = build_idle_notification_blocks("Pick:", "proj", "sess", &[choice]);
    let elements = blocks
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["type"] == "actions")
        .unwrap()["elements"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(
        elements[0]["text"]["text"],
        "あいうえおかきくけこさしすせそたちつてと..."
    );
}
