use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
struct LockData {
    pid: u32,
    started_at: String,
    prompt: String,
    last_event_at: String,
}

/// `--prompt` セッションの排他ロック。
/// `.vibepod/prompt.lock` ファイルを管理する。
/// Drop 時にロックを自動解放するが、SIGKILL 等では Drop が走らないため
/// stale 検知（PID 生存チェック）が主要な安全弁となる。
pub struct PromptLock {
    path: PathBuf,
}

impl PromptLock {
    pub fn acquire(vibepod_dir: PathBuf, prompt: String) -> Result<Self> {
        let path = vibepod_dir.join("prompt.lock");
        fs::create_dir_all(&vibepod_dir)?;

        let now = chrono::Local::now().to_rfc3339();
        let data = LockData {
            pid: std::process::id(),
            started_at: now.clone(),
            prompt,
            last_event_at: now,
        };
        let json = serde_json::to_string_pretty(&data)?;
        // create_new でアトミックに作成（同時起動時の TOCTOU を防止）
        use std::io::Write;
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(json.as_bytes())
                    .context("Failed to write prompt.lock")?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // ファイルが既に存在する: 別プロセスが同時にロックを取得した
                anyhow::bail!(
                    "セッション実行中です（ロックファイルが既に存在します）\n停止するには: vibepod stop"
                );
            }
            Err(e) => {
                return Err(anyhow::anyhow!(e).context("Failed to create prompt.lock"));
            }
        }

        Ok(Self { path })
    }

    // 他モジュールが &PathBuf で渡すため、&Path への変換は行わない
    #[allow(clippy::ptr_arg)]
    pub fn check(vibepod_dir: &PathBuf) -> Option<u32> {
        let path = vibepod_dir.join("prompt.lock");
        let content = fs::read_to_string(&path).ok()?;
        let data: LockData = match serde_json::from_str(&content) {
            Ok(d) => d,
            Err(_) => {
                // パース失敗（破損ファイル）: stale として削除
                // update_last_event() 中の SIGKILL で truncated JSON が残るケースに対応
                fs::remove_file(&path).ok();
                return None;
            }
        };

        if is_process_alive(data.pid) {
            Some(data.pid)
        } else {
            fs::remove_file(&path).ok();
            None
        }
    }

    pub fn update_last_event(&self) -> Result<()> {
        let content = fs::read_to_string(&self.path).context("Failed to read prompt.lock")?;
        let mut data: LockData =
            serde_json::from_str(&content).context("Failed to parse prompt.lock")?;
        data.last_event_at = chrono::Local::now().to_rfc3339();
        let json = serde_json::to_string_pretty(&data)?;
        // アトミックに書き換え: 一時ファイルに書いてからリネーム
        // fs::write は truncate → write の 2 ステップで、途中で check() が読むと
        // パース失敗 → stale 扱いでロックが消える競合が起きる
        let tmp_path = self.path.with_extension("lock.tmp");
        fs::write(&tmp_path, json).context("Failed to write prompt.lock.tmp")?;
        fs::rename(&tmp_path, &self.path).context("Failed to rename prompt.lock.tmp")?;
        Ok(())
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn release(self) {
        // Drop が呼ばれてファイルが削除される
    }
}

impl Drop for PromptLock {
    fn drop(&mut self) {
        fs::remove_file(&self.path).ok();
    }
}

fn is_process_alive(pid: u32) -> bool {
    // SAFETY: kill(pid, 0) はシグナルを送らず、プロセスの存在確認のみを行う。
    // 制約: PID 再利用により、元のプロセスが死んで別プロセスが同じ PID を取得した場合に
    // 誤って「生存」と判定する可能性がある。発生頻度は極めて低く、実用上は許容する。
    let rc = unsafe { libc::kill(pid as i32, 0) };
    if rc == 0 {
        true
    } else {
        // EPERM: プロセスは存在するが権限不足 → 生存として扱う
        matches!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EPERM)
        )
    }
}
