fn main() {
    println!("cargo:rerun-if-changed=icons/icon.ico");
    #[cfg(windows)]
    {
        embed_integration_test_manifest();
        embed_test_manifest();
    }
    tauri_build::build()
}

/// 给 `tests/*.rs` 集成测试目标补同一份 Common Controls v6 清单，**无条件**生效。
///
/// **为什么还要单独来一次**：`src-tauri/tests/` 下的集成测试也走 `tauri::test` 的 mock
/// runtime，因此同样会链进窗口层、同样导入 `comctl32.dll!TaskDialogIndirect`。而
/// `cargo:rustc-link-arg`（上面那个 feature 门控的路径）**不覆盖集成测试目标**——Cargo 的
/// 链接参数作用域是逐目标分开的。
///
/// **为什么可以无条件发出**：`-tests` 只作用于 `tests/*.rs`，经实测**不作用于 bin**。应用
/// 二进制的清单由 `tauri_build` 经 `tauri-winres` → `embed-resource` 的
/// `cargo:rustc-link-arg-bins` 注入，两者作用域不相交，因此不存在「两份清单叠加」的冲突风险，
/// 也就不需要 feature 开关。
///
/// **清单文件必须纯 ASCII**（原因同 [`embed_test_manifest`]）：`mt.exe` 按 ANSI 代码页解析。
#[cfg(windows)]
fn embed_integration_test_manifest() {
    let manifest = std::path::Path::new("windows").join("tests.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());

    let Ok(absolute) = std::fs::canonicalize(&manifest) else {
        println!(
            "cargo:warning=找不到 {}，集成测试二进制将缺少 Common Controls v6 清单",
            manifest.display()
        );
        return;
    };

    // `canonicalize` 在 Windows 上会带 `\\?\` 前缀，link.exe 不认。
    let absolute = absolute.display().to_string();
    let absolute = absolute.strip_prefix(r"\\?\").unwrap_or(&absolute);

    println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-tests=/MANIFESTINPUT:{absolute}");
}

/// 给测试目标补一份 Common Controls v6 清单，仅在 `mock-runtime-tests` feature 开启时生效。
///
/// **为什么需要**：`tauri::test` 的 mock runtime 会把窗口层链进测试二进制，从而导入
/// `comctl32.dll` 的 `TaskDialogIndirect`——那是 Common Controls v6 的专有导出。二进制没有
/// 声明 v6 依赖时，加载器会把它解析到 System32 下的 v5.82，找不到该入口点并报
/// `STATUS_ENTRYPOINT_NOT_FOUND`（`0xc0000139`）。测试进程在跑任何用例之前就死掉，
/// 表现为 `cargo test --lib` 没有任何输出直接失败。
///
/// **为什么用 `rustc-link-arg` 而不是 `rustc-link-arg-tests`**：后者只作用于 `tests/*.rs`
/// 集成测试目标，对 lib 的单元测试目标完全无效（Cargo 会直接报 "does not have a test
/// target"）。`rustc-link-arg` 是唯一覆盖 lib 单元测试的作用域，代价是它同时作用于 bin，
/// 所以必须配合 feature 开关使用，保证默认构建不会给应用二进制叠加第二份清单。
///
/// **为什么清单文件必须纯 ASCII**：`mt.exe` 不按 XML 声明里的 `encoding="UTF-8"` 解析，
/// 而是按 ANSI 代码页。文件里出现中文会让它报「无法分析请求的 XML 数据」，链接以
/// `LNK1327` 失败。所以 `windows/tests.manifest` 里不写中文注释，解释放在这里。
#[cfg(windows)]
fn embed_test_manifest() {
    if std::env::var_os("CARGO_FEATURE_MOCK_RUNTIME_TESTS").is_none() {
        return;
    }

    let manifest = std::path::Path::new("windows").join("tests.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());

    let Ok(absolute) = std::fs::canonicalize(&manifest) else {
        println!(
            "cargo:warning=mock-runtime-tests 已开启，但找不到 {}，测试二进制将缺少 Common Controls v6 清单",
            manifest.display()
        );
        return;
    };

    // `canonicalize` 在 Windows 上会带 `\\?\` 前缀，link.exe 不认。
    let absolute = absolute.display().to_string();
    let absolute = absolute.strip_prefix(r"\\?\").unwrap_or(&absolute);

    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{absolute}");
}
