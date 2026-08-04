use anyhow::{Context, Result};
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
///
/// profile を追加する際は、この配列への追記に加えて `templates/Dockerfile`
/// へ `AS distro-<p>` と `AS profile-<p>` の2ステージを両方追加すること
/// （`tests/dockerfile_profile_stage_test.rs` がこの配列を読んで両ステージの
/// 存在を静的に検証しており、追加を忘れると `cargo test` の時点で落ちる）。
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
/// - `localhost:5000/vibepod-claude:latest` → `localhost:5000/vibepod-claude-swift:latest`
/// - `localhost:5000/vibepod-claude`（タグなし） → `localhost:5000/vibepod-claude-swift`
///
/// `rsplit_once(':')` は最後の `:` で分割するため、レジストリポート付き
/// かつタグなしの形式（`host:port/name`）だと `:` の後ろがポート番号＋
/// パス（`5000/name`）になり、誤ってポート番号をタグとして書き換えて
/// しまう。`:` の後ろに `/` を含む場合はタグではないと判定し、名前部を
/// 特定できないので全体の末尾へサフィックスを付ける（`host:port/name-swift`
/// のようにレジストリ部を含めて丸ごと扱う）。
///
/// digest 参照（`name@sha256:...`）は非対応。`docker build -t` のタグとして
/// 指定できる形式ではなく、呼び出し元（`image_for_profile` を呼ぶ経路）が
/// 生成・管理するイメージ名は常にタグ形式のため、この関数の入力として渡り
/// 得ない前提（YAGNI: 到達不能な入力へのガードは追加しない）。
pub fn image_for_profile(base_image: &str, profile: &str) -> String {
    match base_image.rsplit_once(':') {
        Some((name, tag)) if !tag.contains('/') => format!("{}-{}:{}", name, profile, tag),
        _ => format!("{}-{}", base_image, profile),
    }
}

impl VibepodConfig {
    /// プロジェクト設定 → グローバル設定の順でマージした設定を返す
    pub fn load(project_dir: &Path, global_config_dir: &Path) -> Result<Self> {
        let project_config = Self::load_file(&project_dir.join(".vibepod/config.toml"))?;
        let global_config = Self::load_file(&global_config_dir.join("config.toml"))?;

        Ok(Self::merge(project_config, global_config))
    }

    /// `path` の config.toml を読み込む。
    ///
    /// ファイルが存在しない（`ErrorKind::NotFound`）場合のみ「設定なし」の
    /// `Ok(None)` を返す — `.vibepod/config.toml` を置かないプロジェクトは
    /// 正常運用のため。それ以外の読込エラー（権限等）・TOML 構文エラー・
    /// 型エラー（例: `profile = 123`）は握り潰さず、対象パスを含む context
    /// 付きで呼び出し元へ伝播する。
    ///
    /// 旧実装は `read_to_string(path).ok()?` と `toml::from_str(&content).ok()`
    /// を使っており、権限エラーも構文エラーも型エラーも一律「ファイルなし」に
    /// 潰していた。これだと config.toml を typo した運用者に何のエラーも
    /// 出さずに「profile 未指定」として起動してしまい、意図しない default
    /// イメージで実行される事故につながる（CLAUDE.md のエラー握り潰し禁止に
    /// も反する）。
    fn load_file(path: &Path) -> Result<Option<VibepodConfig>> {
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(e).with_context(|| format!("Failed to read {}", path.display()));
            }
        };
        let config: VibepodConfig = toml::from_str(&content)
            .with_context(|| format!("Failed to parse {} as TOML", path.display()))?;
        Ok(Some(config))
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

    // レジストリポート形式（`host:port/name[:tag]`）は `:` を含むが、
    // 最後の `:` 以降がタグとは限らない（`/` を含む場合はポート番号）。
    // タグ扱いして誤ってポート部を書き換えないことを確認する。

    #[test]
    fn image_for_profile_with_registry_port_and_tag_inserts_before_tag() {
        assert_eq!(
            image_for_profile("localhost:5000/vibepod-claude:latest", "swift"),
            "localhost:5000/vibepod-claude-swift:latest"
        );
    }

    #[test]
    fn image_for_profile_with_registry_port_and_no_tag_appends_suffix() {
        assert_eq!(
            image_for_profile("localhost:5000/vibepod-claude", "swift"),
            "localhost:5000/vibepod-claude-swift"
        );
    }

    // F1（フル再レビュー Major 指摘）: `load_file` の握り潰し解消。
    //
    // 旧実装は `read_to_string(path).ok()?` と `toml::from_str(&content).ok()`
    // により、権限エラー・TOML 構文エラー・型エラー（`profile = 123` 等）が
    // すべて「ファイルなし（設定なし）」に化けていた。運用者が config.toml を
    // typo しても vibepod が黙って「profile 未指定」として起動してしまい、
    // 意図しない default イメージで走る事故につながる。ファイル不存在
    // （`ErrorKind::NotFound`）のみ `Ok(None)` とし、それ以外の読込エラー・
    // 解析エラーは対象パスを含めて呼び出し元へ伝播することを、プロジェクト側・
    // グローバル側の両方、構文エラー・型エラーの両方で固定する。

    fn write_raw(dir: &Path, content: &str) {
        std::fs::create_dir_all(dir).expect("failed to create config dir");
        std::fs::write(dir.join("config.toml"), content).expect("failed to write config.toml");
    }

    #[test]
    fn load_fails_with_path_context_on_invalid_toml_syntax_in_project_config() {
        let project = tempfile::tempdir().expect("failed to create project tempdir");
        let global = tempfile::tempdir().expect("failed to create global tempdir");
        // 閉じられていない `[run` — TOML として構文エラー。
        write_raw(
            &project.path().join(".vibepod"),
            "[run\nprofile = \"swift\"\n",
        );

        let err = VibepodConfig::load(project.path(), global.path())
            .expect_err("syntax error in project config.toml must not be swallowed as 'no config'");
        let message = format!("{err:#}");
        assert!(
            message.contains("config.toml"),
            "error must name the offending file: {message}"
        );
    }

    #[test]
    fn load_fails_with_path_context_on_type_error_in_project_config() {
        let project = tempfile::tempdir().expect("failed to create project tempdir");
        let global = tempfile::tempdir().expect("failed to create global tempdir");
        // `profile` は文字列型のはずが整数 — TOML としては valid、型が不一致。
        write_raw(&project.path().join(".vibepod"), "[run]\nprofile = 123\n");

        let err = VibepodConfig::load(project.path(), global.path())
            .expect_err("type error in project config.toml must not be swallowed as 'no config'");
        let message = format!("{err:#}");
        assert!(
            message.contains("config.toml"),
            "error must name the offending file: {message}"
        );
    }

    #[test]
    fn load_fails_with_path_context_on_invalid_toml_syntax_in_global_config() {
        let project = tempfile::tempdir().expect("failed to create project tempdir");
        let global = tempfile::tempdir().expect("failed to create global tempdir");
        write_raw(&project.path().join(".vibepod"), "[run]\nlang = \"rust\"\n");
        write_raw(global.path(), "[run\nprofile = \"swift\"\n");

        let err = VibepodConfig::load(project.path(), global.path())
            .expect_err("syntax error in global config.toml must not be swallowed as 'no config'");
        let message = format!("{err:#}");
        assert!(
            message.contains("config.toml"),
            "error must name the offending file: {message}"
        );
    }

    #[test]
    fn load_fails_with_path_context_on_type_error_in_global_config() {
        let project = tempfile::tempdir().expect("failed to create project tempdir");
        let global = tempfile::tempdir().expect("failed to create global tempdir");
        write_raw(&project.path().join(".vibepod"), "[run]\nlang = \"rust\"\n");
        write_raw(global.path(), "[run]\nprofile = 123\n");

        let err = VibepodConfig::load(project.path(), global.path())
            .expect_err("type error in global config.toml must not be swallowed as 'no config'");
        let message = format!("{err:#}");
        assert!(
            message.contains("config.toml"),
            "error must name the offending file: {message}"
        );
    }
}
