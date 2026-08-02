use anyhow::Result;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Default)]
/// プロジェクト設定とグローバル設定をマージした結果を保持する。プロジェクト設定が優先される。
pub struct VibepodConfig {
    pub run: Option<RunConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RunConfig {
    pub lang: Option<String>,
    pub prompt_idle_timeout: Option<u64>,
    /// 言語プロファイル。現状の有効値は `"swift"` のみ（[`validate_profile`]
    /// で検証する）。`None` は現行どおりデフォルトイメージを使う。
    pub profile: Option<String>,
}

/// `profile` に指定できる値の一覧。エラーメッセージにそのまま埋め込む
/// ため、増える場合はここへ追記するだけで済むようにしている。
pub const VALID_PROFILES: &[&str] = &["swift"];

/// `.vibepod/config.toml` の `[run] profile` を検証する純関数。
///
/// `None`（未指定）は常に有効。`Some` の場合は [`VALID_PROFILES`] に含まれる
/// 値のみを許可する。無効な値はどの選択肢が有効かを含むメッセージで
/// エラーにする — 運用者がその場で config.toml を直せるようにするため。
pub fn validate_profile(profile: &Option<String>) -> Result<()> {
    match profile {
        None => Ok(()),
        Some(p) if VALID_PROFILES.contains(&p.as_str()) => Ok(()),
        Some(p) => Err(anyhow::anyhow!(
            "invalid profile '{}' in [run] (config.toml): valid values are: {}",
            p,
            VALID_PROFILES.join(", ")
        )),
    }
}

/// `profile` 指定時のイメージ名を導出する純関数。
///
/// ベースイメージのタグは維持したまま、名前部（タグより前）へ
/// `-<profile>` を挿入する。
///
/// - `vibepod-claude:latest` + `"swift"` → `vibepod-claude-swift:latest`
/// - `vibepod-claude`（タグなし） → `vibepod-claude-swift`
///
/// 呼び出し元は vibepod が生成・管理するイメージ名（`vibepod-<agent>[:<tag>]`）
/// のみを渡す想定。レジストリポート（`host:5000/name`）のような `:` を含む
/// が実際にはタグではない形式は現行の呼び出し元では発生しないため扱わない。
pub fn image_for_profile(base_image: &str, profile: &str) -> String {
    match base_image.rsplit_once(':') {
        Some((name, tag)) => format!("{}-{}:{}", name, profile, tag),
        None => format!("{}-{}", base_image, profile),
    }
}

impl VibepodConfig {
    /// プロジェクト設定 → グローバル設定の順でマージした設定を返す
    pub fn load(project_dir: &Path, global_config_dir: &Path) -> Result<Self> {
        let project_config = Self::load_file(&project_dir.join(".vibepod/config.toml"));
        let global_config = Self::load_file(&global_config_dir.join("config.toml"));

        Ok(Self::merge(project_config, global_config))
    }

    fn load_file(path: &Path) -> Option<VibepodConfig> {
        let content = std::fs::read_to_string(path).ok()?;
        toml::from_str(&content).ok()
    }

    fn merge(project: Option<Self>, global: Option<Self>) -> Self {
        match (project, global) {
            (Some(p), Some(g)) => {
                // フィールド単位でディープマージ（プロジェクト優先、なければグローバル）
                let lang = p
                    .run
                    .as_ref()
                    .and_then(|r| r.lang.clone())
                    .or(g.run.as_ref().and_then(|r| r.lang.clone()));
                let prompt_idle_timeout = p
                    .run
                    .as_ref()
                    .and_then(|r| r.prompt_idle_timeout)
                    .or(g.run.as_ref().and_then(|r| r.prompt_idle_timeout));
                let profile = p
                    .run
                    .as_ref()
                    .and_then(|r| r.profile.clone())
                    .or(g.run.as_ref().and_then(|r| r.profile.clone()));

                VibepodConfig {
                    run: if p.run.is_some() || g.run.is_some() {
                        Some(RunConfig {
                            lang,
                            prompt_idle_timeout,
                            profile,
                        })
                    } else {
                        None
                    },
                }
            }
            (Some(p), None) => p,
            (None, Some(g)) => g,
            (None, None) => Self::default(),
        }
    }

    pub fn lang(&self) -> Option<String> {
        self.run.as_ref().and_then(|r| r.lang.clone())
    }

    pub fn profile(&self) -> Option<String> {
        self.run.as_ref().and_then(|r| r.profile.clone())
    }

    pub fn prompt_idle_timeout(&self) -> u64 {
        self.run
            .as_ref()
            .and_then(|r| r.prompt_idle_timeout)
            .unwrap_or(300)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(dir: &Path, profile: Option<&str>) {
        std::fs::create_dir_all(dir).expect("failed to create config dir");
        let content = match profile {
            Some(p) => format!("[run]\nprofile = \"{}\"\n", p),
            None => "[run]\nlang = \"rust\"\n".to_string(),
        };
        std::fs::write(dir.join("config.toml"), content).expect("failed to write config.toml");
    }

    // テスト計画 第5節 項目1: RunConfig.profile のパースとマージ
    // （プロジェクト優先・グローバルのみ・未指定の3パターン）

    #[test]
    fn profile_project_overrides_global() {
        let project = tempfile::tempdir().expect("failed to create project tempdir");
        let global = tempfile::tempdir().expect("failed to create global tempdir");
        write_config(&project.path().join(".vibepod"), Some("swift"));
        write_config(global.path(), Some("kotlin"));

        let config =
            VibepodConfig::load(project.path(), global.path()).expect("failed to load config");

        assert_eq!(config.profile(), Some("swift".to_string()));
    }

    #[test]
    fn profile_falls_back_to_global_when_project_unset() {
        let project = tempfile::tempdir().expect("failed to create project tempdir");
        let global = tempfile::tempdir().expect("failed to create global tempdir");
        write_config(&project.path().join(".vibepod"), None);
        write_config(global.path(), Some("swift"));

        let config =
            VibepodConfig::load(project.path(), global.path()).expect("failed to load config");

        assert_eq!(config.profile(), Some("swift".to_string()));
    }

    #[test]
    fn profile_is_none_when_unset_everywhere() {
        let project = tempfile::tempdir().expect("failed to create project tempdir");
        let global = tempfile::tempdir().expect("failed to create global tempdir");
        write_config(&project.path().join(".vibepod"), None);
        write_config(global.path(), None);

        let config =
            VibepodConfig::load(project.path(), global.path()).expect("failed to load config");

        assert_eq!(config.profile(), None);
    }

    // テスト計画 第5節 項目2: profile 検証（"swift" は許可、それ以外はエラー）

    #[test]
    fn validate_profile_accepts_none() {
        assert!(validate_profile(&None).is_ok());
    }

    #[test]
    fn validate_profile_accepts_swift() {
        assert!(validate_profile(&Some("swift".to_string())).is_ok());
    }

    #[test]
    fn validate_profile_rejects_unknown_value() {
        let err = validate_profile(&Some("kotlin".to_string()))
            .expect_err("kotlin is not a valid profile");
        assert!(
            err.to_string().contains("swift"),
            "error message should list valid values: {}",
            err
        );
    }

    #[test]
    fn validate_profile_rejects_empty_string() {
        assert!(validate_profile(&Some(String::new())).is_err());
    }

    // テスト計画 第5節 項目3: image_for_profile（タグあり／タグなし）

    #[test]
    fn image_for_profile_with_tag_inserts_before_tag() {
        assert_eq!(
            image_for_profile("vibepod-claude:latest", "swift"),
            "vibepod-claude-swift:latest"
        );
    }

    #[test]
    fn image_for_profile_without_tag_appends_suffix() {
        assert_eq!(
            image_for_profile("vibepod-claude", "swift"),
            "vibepod-claude-swift"
        );
    }
}
