/// Parse env file lines into key=value pairs, skipping comments and empty lines
fn parse_env_file(content: &str) -> Vec<(String, String)> {
    content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .filter_map(|line| {
            let trimmed = line.trim();
            let (key, value) = trimmed.split_once('=')?;
            let value = value.trim_matches('"').trim_matches('\'');
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

fn has_op_references(vars: &[(String, String)]) -> bool {
    vars.iter().any(|(_, v)| v.starts_with("op://"))
}

#[test]
fn test_parse_env_file_basic() {
    let content = r#"
# Database config
DB_HOST=localhost
DB_PASSWORD="op://ai-agents/VibePod Test/password"
API_KEY='some-key'
"#;
    let vars = parse_env_file(content);
    assert_eq!(vars.len(), 3);
    assert_eq!(vars[0], ("DB_HOST".into(), "localhost".into()));
    assert_eq!(
        vars[1],
        (
            "DB_PASSWORD".into(),
            "op://ai-agents/VibePod Test/password".into()
        )
    );
    assert_eq!(vars[2], ("API_KEY".into(), "some-key".into()));
}

#[test]
fn test_has_op_references() {
    let with_op = vec![("KEY".into(), "op://vault/item/field".into())];
    assert!(has_op_references(&with_op));

    let without_op = vec![("KEY".into(), "plain-value".into())];
    assert!(!has_op_references(&without_op));
}

#[test]
fn test_parse_env_file_empty_and_comments() {
    let content = r#"
# Only comments

# Another comment
"#;
    let vars = parse_env_file(content);
    assert!(vars.is_empty());
}
