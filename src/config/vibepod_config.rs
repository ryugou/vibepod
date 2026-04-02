use anyhow::Result;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct VibepodConfig {
    pub review: Option<ReviewConfig>,
    pub run: Option<RunConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ReviewConfig {
    pub reviewers: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RunConfig {
    pub lang: Option<String>,
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
                let reviewers = p
                    .review
                    .as_ref()
                    .and_then(|r| r.reviewers.clone())
                    .or(g.review.as_ref().and_then(|r| r.reviewers.clone()));
                let lang = p
                    .run
                    .as_ref()
                    .and_then(|r| r.lang.clone())
                    .or(g.run.as_ref().and_then(|r| r.lang.clone()));

                VibepodConfig {
                    review: if p.review.is_some() || g.review.is_some() {
                        Some(ReviewConfig { reviewers })
                    } else {
                        None
                    },
                    run: if p.run.is_some() || g.run.is_some() {
                        Some(RunConfig { lang })
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

    pub fn reviewers(&self) -> Vec<String> {
        self.review
            .as_ref()
            .and_then(|r| r.reviewers.clone())
            .unwrap_or_default()
    }

    pub fn lang(&self) -> Option<String> {
        self.run.as_ref().and_then(|r| r.lang.clone())
    }
}
