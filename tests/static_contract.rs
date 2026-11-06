//! 源码级静态契约：Windows 收尾、MSRV 与关键调用形式。
//! 这里不执行 `install.ps1`，只检查文本。

const INSTALL_PS1: &str = include_str!("../install.ps1");
const CARGO_TOML: &str = include_str!("../Cargo.toml");
const KEYSTORE: &str = include_str!("../src/keystore.rs");
const PROC: &str = include_str!("../src/proc.rs");
const PATHS: &str = include_str!("../src/paths.rs");

#[test]
fn install_ps1_fails_when_init_fails() {
    let init = INSTALL_PS1
        .find("& $exe init")
        .expect("install.ps1 必须调用 $exe init");
    let after = &INSTALL_PS1[init..];
    let check = after
        .find("$LASTEXITCODE")
        .expect("init 后必须立即检查 $LASTEXITCODE");
    let done = after.find("安装完成").expect("必须报告安装完成");
    assert!(
        check < done,
        "init 退出码检查必须在“安装完成”之前，当前顺序不对"
    );
    let block = after[check..]
        .lines()
        .take(3)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(block.contains("-ne 0"), "必须判断非零退出：{block:?}");
    assert!(block.contains("Fail"), "非零必须走现有 Fail：{block:?}");
}

#[test]
fn cargo_declares_msrv_182_and_fs2() {
    assert!(
        CARGO_TOML.contains("rust-version = \"1.82\""),
        "MSRV 必须声明为 1.82"
    );
    assert!(
        !CARGO_TOML.contains("rust-version = \"1.89\""),
        "不得用提高 MSRV 掩盖 std 文件锁"
    );
    assert!(
        CARGO_TOML.contains("fs2 = \"0.4\""),
        "必须使用 crates.io 的 fs2 0.4，而不是本地维护的锁实现"
    );
    assert!(
        !CARGO_TOML.contains("vendor/fs2"),
        "不得用本地 fs2 shim 替代成熟依赖"
    );
}

#[test]
fn keystore_uses_fully_qualified_fs2_calls() {
    assert!(
        KEYSTORE.contains("use fs2::FileExt;"),
        "必须引入 fs2::FileExt"
    );
    assert!(
        KEYSTORE.contains("<File as FileExt>::lock_exclusive(&file)"),
        "加锁必须使用 fs2 的 lock_exclusive，避免与 1.89 的 std 方法歧义"
    );
    assert!(
        KEYSTORE.contains("<File as FileExt>::unlock(&self.file)"),
        "解锁必须写成 <File as FileExt>::unlock(&self.file)"
    );
    assert!(
        !KEYSTORE.contains("file.lock()"),
        "不得调用 std 的 File::lock"
    );
    assert!(
        !KEYSTORE.contains("self.file.unlock()"),
        "不得调用 std 的 File::unlock"
    );
}

#[test]
fn windows_signal_does_not_kill_process_tree() {
    assert!(
        !PROC.contains("\"/T\""),
        "taskkill 不得使用 /T 杀无关进程树"
    );
}

#[test]
fn paths_tests_module_is_unix_only() {
    assert!(
        PATHS.contains("#[cfg(all(test, unix))]"),
        "仅 Unix 的 paths 测试模块必须整体 cfg 掉，避免 Windows unused import"
    );
}
