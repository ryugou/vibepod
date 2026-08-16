//! `~/.claude/plugins/data` の per-container rw ステージ（1.9.1 で修正）が
//! 実際の docker ランタイム上で成立していることを固定する実機統合テスト。
//!
//! すべて `#[ignore]` を付ける。docker が無い環境（通常の `cargo test`）を
//! 壊さないため。実行するには:
//! `cargo test --test container_integration_test -- --ignored`
//!
//! **背景**: 「親 ro マウント（`~/.claude/plugins/`）の内側に、子 rw マウント
//! （`plugins/data/` の per-container ステージ）を重ねる」という構成は
//! コンテナランタイム依存であり、`ContainerConfig::to_create_args()` の
//! ユニットテスト（`tests/docker_test.rs` の
//! `test_to_create_args_rw_mounts_no_ro_suffix_and_after_extra_mounts`）は
//! 「引数の並び」だけを見ており、docker が実際にその並びどおりネスト
//! マウントを成立させるかは検証できない。1.9.1 の plugins/data 修正は
//! 362 passed のままリグレッションを検知できず素通りした前例があるため、
//! ここで実 docker 上の不変条件として固定する。
//!
//! **マウント構成の導出元**: テスト内でマウント引数をハードコードすると、
//! 本番の `plugins_mount_entries` / `plugins_data_mount_entries` /
//! `ContainerConfig::to_create_args()` が変わってもテストが緑のまま
//! ドリフトする。そのため、ホスト側パス以外（コンテナ側パスを含む）は
//! すべてこれらの本番関数の戻り値から取得する。
//!
//! **「コンテナ側 mountpoint」と「マウント元（source）」は別物**（重要な
//! 区別。詳細は `missing_nested_mount_target_fails_container_creation` の
//! コメント）:
//! - コンテナ側 mountpoint = 親 ro マウントの**ホスト側ソース**の中にある
//!   `data/` サブディレクトリ（`~/.claude/plugins/data`）。ここが無いと
//!   docker はネストマウントの mountpoint をコンテナ rootfs 内に作れず
//!   `EROFS` で起動が失敗する（`prepare_plugins_data_mount` の
//!   `create_dir_all(&host_data_dir)` はこれを防ぐためのもの）。
//! - マウント元（source） = 子 rw マウントのホスト側ソース
//!   （`runtime_dir/plugins-data` 相当）。こちらは ro 制約を受けないため、
//!   存在しなくても docker が自動作成でき、起動可否には影響しない。
//!   このテストファイルでは検証しない（自動作成の有無が docker/OS の
//!   バージョンで変わりうり、フレーキーになるため）。

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;
use vibepod::cli::run::{plugins_data_mount_entries, plugins_mount_entries};
use vibepod::runtime::ContainerConfig;

/// テストで作成したコンテナを、panic 時も含めて必ず削除するガード。
/// `docker run` が (ネストマウント失敗などで) 起動に失敗した場合も
/// コンテナオブジェクト自体は `Created` 状態で残るため、成功可否に
/// 関わらず常に `docker rm -f` を試みる（存在しなければ黙って失敗する
/// だけで無害）。流儀は `tests/docker_test.rs` の `ContainerGuard` に揃えた。
struct ContainerGuard(String);

impl Drop for ContainerGuard {
    fn drop(&mut self) {
        // Drop 中に panic すると、テスト失敗によるアンワインド中であれば
        // 二重 panic でプロセスごと abort し、元の失敗理由（テスト名・assert
        // メッセージ）が一切表示されなくなる。そのため unwrap/expect/assert
        // は使わず、失敗はすべて eprintln で報告するに留める。
        //
        // 削除に失敗しても CI 上でコンテナが残ったまま無言で消えると、後から
        // 「なぜ vibepod-test-* が残っているのか」を誰も追えなくなるため、
        // spawn 失敗・非ゼロ終了のいずれも原因を出力する。ただし
        // `missing_nested_mount_target_fails_container_creation` のように
        // `docker run` 自体が失敗してコンテナが1つも作られなかったケースでは
        // 毎回「No such container」が出て純粋なノイズになるため、これだけは
        // 抑制する。
        match Command::new("docker").args(["rm", "-f", &self.0]).output() {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stderr.contains("No such container") {
                    eprintln!(
                        "警告: テストコンテナ {} の削除 (`docker rm -f`) が失敗しました \
                         (exit: {:?}). stderr: {}. 手動で `docker rm -f {}` を実行してください。",
                        self.0,
                        output.status.code(),
                        stderr.trim(),
                        self.0
                    );
                }
            }
            Err(err) => {
                eprintln!(
                    "警告: テストコンテナ {} の削除コマンド (`docker rm -f`) の起動自体に \
                     失敗しました: {err}. 手動で `docker rm -f {}` を実行してください。",
                    self.0, self.0
                );
            }
        }
    }
}

/// ネストマウント検証用のコンテナ1個分のセットアップ結果。
///
/// フィールド宣言順が Drop 順を決める（Rust は構造体フィールドを宣言順に
/// drop する）。`guard` を先頭に置くことで、`docker rm -f` が host 側
/// tempdir 群の削除より必ず先に走るようにしている（削除順序自体が
/// 不変条件に影響するわけではないが、host ディレクトリが消えた後の
/// コンテナに対して `docker rm -f` を投げる不自然な状態を避けるため）。
struct NestedMountFixture {
    guard: ContainerGuard,
    _workspace: TempDir,
    /// 親 ro マウントのホスト側ソース（`~/.claude/plugins` 相当）
    parent: TempDir,
    /// 子 rw マウントのホスト側ソース（`runtime_dir/plugins-data` 相当）
    _stage: TempDir,
    container_name: String,
    /// `plugins_mount_entries` が返した1本目（`$HOME` 経由）のコンテナ側パス
    /// （例: `/home/vibepod/.claude/plugins`）
    container_plugins_path: String,
    /// `plugins_data_mount_entries` が返した1本目（`$HOME` 経由）のコンテナ側
    /// パス（例: `/home/vibepod/.claude/plugins/data`）
    container_plugins_data_path: String,
    /// `plugins_mount_entries` が返した2本目（`installed_plugins.json` の
    /// ホスト絶対パスを解決するための再マウント）のコンテナ側パス。
    /// `container_home == /home/vibepod` の場合、1本目と重複するため
    /// 2本目は生成されず `None` になる。
    container_plugins_path_absolute: Option<String>,
    /// `plugins_data_mount_entries` が返した2本目（ホスト絶対パス側）の
    /// コンテナ側パス。`container_plugins_path_absolute` と同じ条件で
    /// `None` になる。
    container_plugins_data_path_absolute: Option<String>,
    /// `docker run -d ...`（`ContainerConfig::to_create_args()` の実行結果）
    run_output: Output,
}

/// ネストマウント構成のコンテナを1個立てる。
///
/// - `seed_parent_data`: 親ホストディレクトリ配下に `data/` サブディレクトリ
///   を事前作成するか。`false` にすると、ro マウントの内側にネストマウント
///   の mountpoint が存在しない状態を作れる（不変条件5の検証用）。
/// - `seed_host_marker`: `seed_parent_data` が `true` の場合のみ有効。
///   `data/` の中に一意なマーカーファイルを事前に置く（不変条件3の検証用）。
/// - `container_home`: `plugins_mount_entries` / `plugins_data_mount_entries`
///   に渡すコンテナ側 HOME。既存の不変条件1〜5は `/home/vibepod`
///   （コンテナのデフォルト HOME と同じ値）を渡す前提で書かれており、その
///   場合 `plugins_mount_entries` / `plugins_data_mount_entries` は
///   「installed_plugins.json の絶対パス解決用」の2本目のエントリを追加せず
///   1本だけ返す（1本目と2本目のコンテナ側パスが一致するため）。
///   `/home/vibepod` 以外を渡すと2本目が追加され、そちらは
///   `container_plugins_path_absolute` /
///   `container_plugins_data_path_absolute` から取り出せる（不変条件6の
///   検証用）。**ホストの実 `$HOME` は絶対に使わない**こと。テストの合否が
///   実行環境に依存してしまうため、`/home/vibepod` 以外を渡す場合は合成パス
///   （例: `/opt/vibepod-test-hosthome`）を使う。
fn spawn_nested_mount_fixture(
    name_suffix: &str,
    seed_parent_data: bool,
    seed_host_marker: bool,
    container_home: &Path,
) -> NestedMountFixture {
    let workspace = tempfile::tempdir().expect("failed to create workspace tempdir");
    let parent = tempfile::tempdir().expect("failed to create parent (plugins) tempdir");
    let stage = tempfile::tempdir().expect("failed to create stage (plugins/data) tempdir");

    if seed_parent_data {
        let data_dir = parent.path().join("data");
        fs::create_dir_all(&data_dir).expect("failed to create parent/data dir");
        if seed_host_marker {
            fs::write(
                data_dir.join("host_marker.txt"),
                "must not be visible from inside the container's child rw mount",
            )
            .expect("failed to write host marker file");
        }
    }

    let extra_mounts = plugins_mount_entries(&parent.path().to_string_lossy(), container_home);
    let rw_mounts = plugins_data_mount_entries(&stage.path().to_string_lossy(), container_home);

    // 期待エントリ本数は `container_home` に応じて変わる。本番の
    // `plugins_mount_entries` / `plugins_data_mount_entries` は、`$HOME` 経由
    // のコンテナ側パス（固定で `/home/vibepod/.claude/plugins[/data]`）と
    // `installed_plugins.json` の絶対パス解決用パス（`container_home` 基準）
    // が一致する場合にのみ2本目を省く。`container_home == /home/vibepod`
    // ならこの2つが同一パスになるため1本、それ以外なら別パスになるため2本
    // 返る。下記の `/home/vibepod` は本番の非 pub 定数
    // `DEFAULT_PLUGINS_CONTAINER_PATH` / `DEFAULT_PLUGINS_DATA_CONTAINER_PATH`
    // （src/cli/run/mod.rs）の値をテスト側にリテラルで書き写したものであり、
    // 分岐条件そのものを再現しているわけではない。これらの定数はコンテナ側
    // の内部実装パスであり、公開 API に広げたくないため `pub` にはせず、
    // このテストでは意図的にリテラルを使っている。値が変わった場合に
    // テストが自動追随することはなく、`extra_mounts.len()` /
    // `rw_mounts.len()` が期待本数と一致しなくなり、直後の assert が失敗
    // することで変更が検知される。
    let expected_entry_count = if container_home == Path::new("/home/vibepod") {
        1
    } else {
        2
    };
    assert_eq!(
        extra_mounts.len(),
        expected_entry_count,
        "test setup assumption violated: expected plugins_mount_entries to return {} \
         entry/entries for container_home == {:?}, got {:?}",
        expected_entry_count,
        container_home,
        extra_mounts
    );
    assert_eq!(
        rw_mounts.len(),
        expected_entry_count,
        "test setup assumption violated: expected plugins_data_mount_entries to return {} \
         entry/entries for container_home == {:?}, got {:?}",
        expected_entry_count,
        container_home,
        rw_mounts
    );
    let container_plugins_path = extra_mounts[0].1.clone();
    let container_plugins_data_path = rw_mounts[0].1.clone();
    let container_plugins_path_absolute =
        extra_mounts.get(1).map(|(_, container)| container.clone());
    let container_plugins_data_path_absolute =
        rw_mounts.get(1).map(|(_, container)| container.clone());

    let container_name = format!(
        "vibepod-test-nested-mount-{}-{}",
        name_suffix,
        std::process::id()
    );
    let guard = ContainerGuard(container_name.clone());

    let config = ContainerConfig {
        image: "alpine:3".to_string(),
        container_name: container_name.clone(),
        workspace_path: workspace.path().to_string_lossy().to_string(),
        claude_json: None,
        codex_dir: None,
        gitconfig: None,
        env_vars: vec![],
        network_disabled: false,
        extra_mounts,
        rw_mounts,
        labels: HashMap::new(),
    };

    let run_output = Command::new("docker")
        .args(config.to_create_args())
        .output()
        .expect("failed to spawn `docker run`");

    NestedMountFixture {
        guard,
        _workspace: workspace,
        parent,
        _stage: stage,
        container_name,
        container_plugins_path,
        container_plugins_data_path,
        container_plugins_path_absolute,
        container_plugins_data_path_absolute,
        run_output,
    }
}

/// `docker exec <name> <args>` を実行する。
fn docker_exec(name: &str, args: &[&str]) -> Output {
    let mut full_args = vec!["exec", name];
    full_args.extend_from_slice(args);
    Command::new("docker")
        .args(&full_args)
        .output()
        .expect("failed to spawn `docker exec`")
}

fn assert_container_started(fx: &NestedMountFixture) {
    assert!(
        fx.run_output.status.success(),
        "docker run should succeed once parent/data pre-exists on host (container: {}). \
         stderr: {}",
        fx.guard.0,
        String::from_utf8_lossy(&fx.run_output.stderr)
    );
}

/// 不変条件1: 親 ro マウントの内側に子 rw マウントを重ねたとき、子が rw になる。
#[test]
#[ignore]
fn nested_child_mount_is_writable() {
    let fx = spawn_nested_mount_fixture("rw-child", true, false, Path::new("/home/vibepod"));
    assert_container_started(&fx);

    let path = format!("{}/rw_check.txt", fx.container_plugins_data_path);
    let out = docker_exec(&fx.container_name, &["touch", &path]);
    assert!(
        out.status.success(),
        "writing inside the nested rw mount ({path}) should succeed, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// 不変条件2: 子を rw で重ねても、親自体は ro のまま。
#[test]
#[ignore]
fn parent_mount_stays_read_only() {
    let fx = spawn_nested_mount_fixture("ro-parent", true, false, Path::new("/home/vibepod"));
    assert_container_started(&fx);

    // data/ の外側、親マウント直下への書き込みは ro のため拒否されるはず。
    let path = format!("{}/should_be_blocked.txt", fx.container_plugins_path);
    let out = docker_exec(&fx.container_name, &["touch", &path]);
    assert!(
        !out.status.success(),
        "writing directly under the parent ro mount ({path}) must fail, but it succeeded"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.to_lowercase().contains("read-only file system"),
        "expected a read-only-filesystem error, got: {stderr}"
    );
}

/// 不変条件3: ホスト側の親ディレクトリ（`data/` 配下）に置いた一意なファイルが、
/// コンテナ内の子マウント先からは見えない（子 rw マウントが上書きしているため）。
#[test]
#[ignore]
fn host_parent_content_is_hidden_by_child_mount() {
    let fx = spawn_nested_mount_fixture("hidden-marker", true, true, Path::new("/home/vibepod"));
    assert_container_started(&fx);

    let path = format!("{}/host_marker.txt", fx.container_plugins_data_path);
    let out = docker_exec(&fx.container_name, &["test", "-e", &path]);
    assert!(
        !out.status.success(),
        "host parent's data/host_marker.txt must NOT be visible at {path} from inside the \
         container (the child rw mount should hide the parent's original data/ contents), \
         but `test -e` reported it exists"
    );
}

/// 不変条件4: コンテナ内で子マウント先に書いたファイルが、ホスト側の親ディレクトリ
/// （`data/`）には現れない（書き込みは子マウントのソース側に着地するはず）。
#[test]
#[ignore]
fn container_writes_do_not_leak_to_host_parent() {
    let fx = spawn_nested_mount_fixture("no-leak", true, false, Path::new("/home/vibepod"));
    assert_container_started(&fx);

    let container_path = format!("{}/from_container.txt", fx.container_plugins_data_path);
    let write = docker_exec(
        &fx.container_name,
        &["sh", "-c", &format!("echo written > {container_path}")],
    );
    assert!(
        write.status.success(),
        "writing via the nested rw mount should succeed, stderr: {}",
        String::from_utf8_lossy(&write.stderr)
    );

    let host_parent_data_file = fx.parent.path().join("data").join("from_container.txt");
    assert!(
        !host_parent_data_file.exists(),
        "a file written inside the container's child rw mount must not appear under the host \
         parent dir at {} (that would mean the rw mount silently landed on the parent's ro \
         source instead of its own stage)",
        host_parent_data_file.display()
    );

    // 陽性対照: 書き込みが本当に発生したこと自体は、子マウントのソース側
    // (`_stage`) に現れることで確認する。`_stage` は fixture の drop-order
    // コメントの都合でアンダースコア接頭辞にしているが未使用ではないため、
    // ここでは直接パスを再構築せずフィールド経由でアクセスする。
    let host_stage_file = fx._stage.path().join("from_container.txt");
    assert!(
        host_stage_file.exists(),
        "expected the write to land in the host stage dir ({}) — if this fails, the write \
         went somewhere unexpected and the negative assertion above is not meaningful",
        host_stage_file.display()
    );
}

/// 不変条件5: 親 ro マウントの**ホスト側ソース**に `data/` サブディレクトリ
/// （= ネストマウントのコンテナ側 mountpoint の実体）が存在しない場合、
/// コンテナ作成そのものが失敗する。
///
/// **メカニズム**: 子 rw マウントを適用する時点で、コンテナ側の
/// mountpoint（`container_plugins_data_path`）は既に ro マウント済みの親の
/// 中にある。ここに対応するディレクトリがホスト側（親ソースの `data/`）に
/// 無いと、docker/runc はコンテナ rootfs 内にマウントポイントを作ろうとして
/// 親が read-only であるため失敗する（`EROFS`）。
///
/// `prepare_plugins_data_mount`（`src/cli/run/mod.rs`）が
/// `create_dir_all(&host_data_dir)` でホスト側 `~/.claude/plugins/data` を
/// 事前作成しているのは、まさにこの起動失敗を防ぐためである。この mkdir が
/// 無いと、`~/.claude/plugins/data` を一度も作ったことがないユーザーは
/// コンテナが**起動すらしない**。
///
/// これは、子 rw マウントの**ソース側**（`ensure_plugins_data_stage_dir` が
/// `0700` を強制している `runtime_dir/plugins-data`）とは別の不変条件である。
/// ソース側が存在しない場合は docker が自動的にホストディレクトリを作成でき
/// （親のような read-only 制約を受けないため）、起動可否には影響しない。
/// 自動作成の有無は docker/OS のバージョンでも変わりうるため、ソース側の
/// 欠如はこのテストファイルでは検証しない。
///
/// 期待する失敗の判定は「`docker run` が非0で終了する」ことと「stderr に
/// `read-only file system` という文言が含まれる」ことに留める。runc の
/// エラーメッセージ全文（ファイルパスや overlay2 の内部パスを含む）を
/// 固定すると runc のバージョン更新だけでテストが壊れるため。
#[test]
#[ignore]
fn missing_nested_mount_target_fails_container_creation() {
    let fx = spawn_nested_mount_fixture("missing-target", false, false, Path::new("/home/vibepod"));

    assert!(
        !fx.run_output.status.success(),
        "docker run must fail when the host directory backing the nested mountpoint \
         (parent/data) does not exist, but it succeeded. stdout: {}",
        String::from_utf8_lossy(&fx.run_output.stdout)
    );
    let stderr = String::from_utf8_lossy(&fx.run_output.stderr);
    assert!(
        stderr.to_lowercase().contains("read-only file system"),
        "expected the failure to be a read-only-filesystem mountpoint-creation error, got: \
         {stderr}"
    );
}

/// 不変条件6: `installed_plugins.json` のホスト絶対パス解決用に追加される
/// 「ホスト絶対パス側」の2本目のマウントエントリ（`plugins_mount_entries` /
/// `plugins_data_mount_entries` が `container_home != /home/vibepod` のときに
/// 返す2本目）でも、1本目（不変条件1・2・4）と同じ rw/ro 不変条件が成立する。
///
/// **背景**: これまでの不変条件1〜5はすべて `container_home == /home/vibepod`
/// で fixture を組んでおり、その場合 `plugins_mount_entries` /
/// `plugins_data_mount_entries` は2本目のエントリを生成しない（1本目と
/// コンテナ側パスが重複するため）。つまり「installed_plugins.json のホスト
/// 絶対パスを再マウントする」という2本目の分岐は、実 docker 上で一度も
/// 検証されていなかった（`docs/release-checklist.md` で「手動のまま」と
/// 分類されていた）。ここでは合成パス（ホストの実 `$HOME` は使わない）を
/// `container_home` として渡すことで2本目を発生させ、同じ3点
/// （rw で書き込める／親は ro のまま／書き込みが ro 側の親ソースへ漏れず
/// rw 側のステージにだけ着地する）を検証する。
#[test]
#[ignore]
fn nested_mount_invariants_hold_for_absolute_host_path_entry() {
    // ホストの実 $HOME を使うと実行環境（CI と開発機で異なる）にテスト結果が
    // 依存してしまうため、テスト専用の合成パスを使う。
    let synthetic_container_home = Path::new("/opt/vibepod-test-hosthome");
    let fx = spawn_nested_mount_fixture("abs-home", true, false, synthetic_container_home);
    assert_container_started(&fx);

    let child_path = fx.container_plugins_data_path_absolute.as_ref().expect(
        "expected plugins_data_mount_entries to return a second (host-absolute-path) entry \
         when container_home != /home/vibepod",
    );
    let parent_path = fx.container_plugins_path_absolute.as_ref().expect(
        "expected plugins_mount_entries to return a second (host-absolute-path) entry \
         when container_home != /home/vibepod",
    );

    // 1. 子 rw マウント先（ホスト絶対パス側）に書き込める。
    let write_path = format!("{child_path}/from_container_abs.txt");
    let write = docker_exec(
        &fx.container_name,
        &["sh", "-c", &format!("echo written > {write_path}")],
    );
    assert!(
        write.status.success(),
        "writing inside the nested rw mount (host-absolute-path side) at {write_path} should \
         succeed, stderr: {}",
        String::from_utf8_lossy(&write.stderr)
    );

    // 2. 親（ホスト絶対パス側）は read-only のまま。
    let ro_path = format!("{parent_path}/should_be_blocked.txt");
    let ro_attempt = docker_exec(&fx.container_name, &["touch", &ro_path]);
    assert!(
        !ro_attempt.status.success(),
        "writing directly under the parent ro mount (host-absolute-path side) at {ro_path} \
         must fail, but it succeeded"
    );
    let ro_stderr = String::from_utf8_lossy(&ro_attempt.stderr);
    assert!(
        ro_stderr.to_lowercase().contains("read-only file system"),
        "expected a read-only-filesystem error, got: {ro_stderr}"
    );

    // 3. コンテナ内で書いたファイルは、ホスト側の ro 親ソース（parent/data/）
    //    には現れない（rw マウントの source と ro マウントの source は別物の
    //    はずで、混同していればここに漏れる）。
    let host_parent_data_file = fx.parent.path().join("data").join("from_container_abs.txt");
    assert!(
        !host_parent_data_file.exists(),
        "a file written inside the container's child rw mount (host-absolute-path side) must \
         not appear under the host parent dir at {} (that would mean the rw mount silently \
         landed on the parent's ro source instead of its own stage)",
        host_parent_data_file.display()
    );

    // 陽性対照: 書き込みが本当に発生したこと自体は、子マウントのソース側
    // (`_stage`) に現れることで確認する。1本目・2本目とも同じホスト
    // ソース（`plugins_data_mount_entries` の `stage_host` 引数）を共有する
    // ため、着地先は不変条件4と同じ `_stage` ディレクトリになる。
    let host_stage_file = fx._stage.path().join("from_container_abs.txt");
    assert!(
        host_stage_file.exists(),
        "expected the write to land in the host stage dir ({}) — if this fails, the write \
         went somewhere unexpected and the negative assertion above is not meaningful",
        host_stage_file.display()
    );
}
