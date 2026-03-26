use vibepod::bridge::formatter::LlmProvider;

#[test]
fn test_llm_provider_none_from_str() {
    let provider = LlmProvider::from_str("none").unwrap();
    assert_eq!(provider, LlmProvider::None);
}

#[test]
fn test_llm_provider_none_env_key_is_none() {
    let provider = LlmProvider::None;
    assert_eq!(provider.env_key_name(), None);
}

#[tokio::test]
async fn test_formatter_none_strips_ansi() {
    use vibepod::bridge::formatter::Formatter;
    let formatter = Formatter::new(LlmProvider::None, String::new());
    let result = formatter.format("\x1b[31mred text\x1b[0m and normal").await;
    assert!(result.contains("red text"));
    assert!(result.contains("and normal"));
    assert!(!result.contains("\x1b["));
}

#[tokio::test]
async fn test_formatter_none_truncates_long_text() {
    use vibepod::bridge::formatter::Formatter;
    let formatter = Formatter::new(LlmProvider::None, String::new());
    let long_text = "x".repeat(3000);
    let result = formatter.format(&long_text).await;
    assert!(result.chars().count() <= 2005); // 2000 + "...\n" prefix
}
