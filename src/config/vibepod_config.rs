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

                VibepodConfig {
                    run: if p.run.is_some() || g.run.is_some() {
                        Some(RunConfig {
                            lang,
                            prompt_idle_timeout,
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
}
