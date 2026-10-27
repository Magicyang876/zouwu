//! 把图标与版本信息编译进 exe（只对 Windows 目标生效）。
//!
//! 这一步为什么不用 `embed-resource` 之类的 crate：它们最终都要找一个
//! **资源编译器**（`rc.exe` / `windres` / `llvm-rc`）来干活。本机没有 Windows SDK，
//! 但有 LLVM 自带的 `llvm-rc`——那就直接调它，少一层依赖，也少一层"它在找什么"的猜谜。
//!
//! 找不到 `llvm-rc` 时**只警告、不失败**：图标是锦上添花，不该让
//! 一个只想 `cargo test` 的人卡在构建脚本上。

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/zouwu.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // 只有 MSVC 目标能可靠地把 `.res` 链进 exe。
    //
    // gnu 目标下 rustc 用 `rust-lld -flavor gnu`，它把 `.res` 当成“不认识的文件格式”
    // （实测：`zouwu.res: file format not recognized`）。要在 gnu 下嵌资源得先用
    // `windres` 转成 COFF 对象，而本机没有 windres——那就不假装能行：
    // 直接跳过并说明白，同时靠“安装时把 ico 拷到 exe 旁边、快捷方式指向它”
    // 把图标这件事办成（见 `install.rs`）。
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        println!(
            "cargo:warning=skipping exe icon: only the msvc target embeds .res here \
             (gnu needs windres). The installed shortcut still gets the icon from zouwu.ico."
        );
        return;
    }

    let Some(rc_tool) = find_rc() else {
        println!(
            "cargo:warning=no resource compiler (llvm-rc) found; \
             the exe will have no icon/version info. \
             Set LLVM_RC=/path/to/llvm-rc.exe to fix."
        );
        return;
    };

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let ico = manifest_dir.join("../../assets/zouwu.ico");
    if !ico.is_file() {
        println!(
            "cargo:warning={} missing (run `cargo run -p zouwu-ui --example make_icon`)",
            ico.display()
        );
        return;
    }

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("out dir"));
    let rc_path = out_dir.join("zouwu.rc");
    let res_path = out_dir.join("zouwu.res");

    std::fs::write(&rc_path, resource_script(&ico)).expect("write rc");

    // llvm-rc 不做 C 预处理（`#define`/`#include` 都不认），所以 `.rc` 里的
    // 每个数字都由这里直接算好写死。
    match Command::new(&rc_tool)
        .arg("/fo")
        .arg(&res_path)
        .arg(&rc_path)
        .output()
    {
        Ok(out) if out.status.success() => {
            println!("cargo:rustc-link-arg={}", res_path.display());
        }
        Ok(out) => println!(
            "cargo:warning=llvm-rc failed: {}{}",
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(err) => println!("cargo:warning=cannot run {}: {err}", rc_tool.display()),
    }
}

/// 找资源编译器：显式环境变量 → PATH → LLVM 默认安装位置。
fn find_rc() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("LLVM_RC") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let exe = if cfg!(windows) {
        "llvm-rc.exe"
    } else {
        "llvm-rc"
    };
    // PATH 里能直接跑，交给操作系统去解析。
    if Command::new(exe).arg("--version").output().is_ok() {
        return Some(PathBuf::from(exe));
    }
    for dir in [
        r"C:\Program Files\LLVM\bin",
        r"C:\Program Files (x86)\LLVM\bin",
    ] {
        let candidate = Path::new(dir).join(exe);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// 生成 `.rc`：一个图标 + 一条 `VERSIONINFO`。
fn resource_script(ico: &Path) -> String {
    // "0.1.0" → (0, 1, 0, 0)；版本号里带预发布后缀（0.1.0-beta）时忽略后缀。
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut parts = version
        .split(['.', '-', '+'])
        .map(|p| p.parse::<u16>().unwrap_or(0));
    let quad = [
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    ];
    let comma = format!("{},{},{},{}", quad[0], quad[1], quad[2], quad[3]);
    let dotted = format!("{}.{}.{}.{}", quad[0], quad[1], quad[2], quad[3]);
    let ico = ico.display().to_string().replace('\\', "\\\\");

    format!(
        r#"1 ICON "{ico}"

1 VERSIONINFO
FILEVERSION {comma}
PRODUCTVERSION {comma}
FILEOS 0x00040004L
FILETYPE 0x00000001L
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "zouwu"
      VALUE "FileDescription", "zouwu - desktop companion with a shortcut ring"
      VALUE "FileVersion", "{dotted}"
      VALUE "InternalName", "zouwu"
      VALUE "OriginalFilename", "zouwu.exe"
      VALUE "ProductName", "zouwu"
      VALUE "ProductVersion", "{dotted}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0409, 1200
  END
END
"#
    )
}
