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
    /// v2 template mechanism: `--prompt` 時に `--template` が指定されて
    /// いない場合に使うデフォルト template 名。Phase 2 ではフィールドを
    /// 読み取るだけで挙動には使わない（Phase 4 で `effective_template_name`
    /// の拡張時に実際の切り替えに使う）。
    pub default_prompt_template: Option<String>,
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
                let default_prompt_template = p
                    .run
                    .as_ref()
                    .and_then(|r| r.default_prompt_template.clone())
                    .or(g
                        .run
                        .as_ref()
                        .and_then(|r| r.default_prompt_template.clone()));

                VibepodConfig {
                    run: if p.run.is_some() || g.run.is_some() {
                        Some(RunConfig {
                            lang,
                            prompt_idle_timeout,
                            default_prompt_template,
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

    pub fn prompt_idle_timeout(&self) -> u64 {
        self.run
            .as_ref()
            .and_then(|r| r.prompt_idle_timeout)
            .unwrap_or(300)
    }

    /// `--prompt` 時に `--template` 未指定なら使われる予定のデフォルト
    /// template 名。Phase 2 時点ではフィールドは存在するが、実際の挙動
    /// 切り替えは Phase 4 で行われるため呼び出し側はまだ使わない。
    pub fn default_prompt_template(&self) -> Option<String> {
        self.run
            .as_ref()
            .and_then(|r| r.default_prompt_template.clone())
    }
}

/// `<global_config_dir>/config.toml` の `[run] default_prompt_template`
/// 値を書き込む（他のフィールドは保持）。
///
/// `value = Some("name")` の場合は値を設定、`None` の場合はキーを削除。
/// `[run]` セクションが無ければ作成する。config.toml そのものが無ければ
/// 作成する。
///
/// この関数は vibepod が書き込み許可を持つ `<global_config_dir>`
/// （通常 `~/.config/vibepod/`）配下にのみ書き込む。プロジェクト設定
/// （`.vibepod/config.toml`）には書き込まない。
pub fn set_default_prompt_template(global_config_dir: &Path, value: Option<&str>) -> Result<()> {
    let config_path = global_config_dir.join("config.toml");

    // 既存 config.toml を raw Table として読む（unknown sections を保持）
    let mut table: toml::value::Table = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;
        toml::from_str(&content).with_context(|| {
            format!(
                "Failed to parse {} as TOML: fix syntax errors first",
                config_path.display()
            )
        })?
    } else {
        std::fs::create_dir_all(global_config_dir).with_context(|| {
            format!(
                "Failed to create config directory: {}",
                global_config_dir.display()
            )
        })?;
        toml::value::Table::new()
    };

    // [run] セクションを取得または作成
    let run_value = table
        .entry("run".to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
    let run_table = match run_value {
        toml::Value::Table(t) => t,
        _ => {
            anyhow::bail!(
                "Config file {} has [run] set to a non-table value; refusing to overwrite",
                config_path.display()
            );
        }
    };

    match value {
        Some(name) => {
            run_table.insert(
                "default_prompt_template".to_string(),
                toml::Value::String(name.to_string()),
            );
        }
        None => {
            run_table.remove("default_prompt_template");
        }
    }

    let serialized = toml::to_string_pretty(&toml::Value::Table(table))
        .context("Failed to serialize updated config.toml")?;
    std::fs::write(&config_path, serialized)
        .with_context(|| format!("Failed to write {}", config_path.display()))?;

    Ok(())
}
