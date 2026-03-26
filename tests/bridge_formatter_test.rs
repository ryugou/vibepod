use std::str::FromStr;
use vibepod::bridge::formatter::{FormatResult, LlmProvider, detect_choices};

#[test]
fn test_llm_provider_none_from_str() {
    let provider: LlmProvider = "none".parse().unwrap();
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
    assert!(result.text.contains("red text"));
    assert!(result.text.contains("and normal"));
    assert!(!result.text.contains("\x1b["));
}

#[tokio::test]
async fn test_formatter_none_truncates_long_text() {
    use vibepod::bridge::formatter::Formatter;
    let formatter = Formatter::new(LlmProvider::None, String::new());
    let long_text = "x".repeat(3000);
    let result = formatter.format(&long_text).await;
    assert!(result.text.chars().count() <= 2005); // 2000 + "...\n" prefix
}

#[test]
fn test_detect_choices_yes_no() {
    let choices = detect_choices("Do you want to proceed? (y/n)");
    assert_eq!(choices, vec!["yes".to_string(), "no".to_string()]);
}

#[test]
fn test_detect_choices_yes_no_full() {
    let choices = detect_choices("Continue? (yes/no)");
    assert_eq!(choices, vec!["yes".to_string(), "no".to_string()]);
}

#[test]
fn test_detect_choices_abc_paren() {
    let choices = detect_choices("Choose:\nA) Apple\nB) Banana\nC) Cherry");
    assert_eq!(choices, vec!["A".to_string(), "B".to_string(), "C".to_string()]);
}

#[test]
fn test_detect_choices_abc_dash() {
    let choices = detect_choices("- A: Apple\n- B: Banana\n- C: Cherry");
    assert_eq!(choices, vec!["A".to_string(), "B".to_string(), "C".to_string()]);
}

#[test]
fn test_detect_choices_abc_hyphen() {
    let choices = detect_choices("A - Apple\nB - Banana\nC - Cherry");
    assert_eq!(choices, vec!["A".to_string(), "B".to_string(), "C".to_string()]);
}

#[test]
fn test_detect_choices_none() {
    let choices = detect_choices("This is just a status message with no choices.");
    assert!(choices.is_empty());
}

#[test]
fn test_detect_choices_numbered() {
    let choices = detect_choices("Options:\n1. First\n2. Second\n3. Third");
    assert_eq!(choices, vec!["1".to_string(), "2".to_string(), "3".to_string()]);
}

#[tokio::test]
async fn test_formatter_none_returns_format_result() {
    use vibepod::bridge::formatter::Formatter;
    let formatter = Formatter::new(LlmProvider::None, String::new());
    let result = formatter.format("Choose:\nA) Apple\nB) Banana").await;
    assert!(!result.text.is_empty());
    assert_eq!(result.choices, vec!["A", "B"]);
}

#[tokio::test]
async fn test_formatter_none_yes_no() {
    use vibepod::bridge::formatter::Formatter;
    let formatter = Formatter::new(LlmProvider::None, String::new());
    let result = formatter.format("Proceed? (y/n)").await;
    assert_eq!(result.choices, vec!["yes", "no"]);
}

#[tokio::test]
async fn test_formatter_none_no_choices() {
    use vibepod::bridge::formatter::Formatter;
    let formatter = Formatter::new(LlmProvider::None, String::new());
    let result = formatter.format("Processing files...").await;
    assert!(result.choices.is_empty());
}
