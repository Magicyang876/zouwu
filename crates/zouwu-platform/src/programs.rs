//! 本机已安装的程序：清单从哪来、怎么打开。
//!
//! ## 为什么扫开始菜单而不是注册表
//!
//! 注册表里有三种"安装过的程序"，每种都会给出已经卸载/已改名的残留：
//! `App Paths` 只有部分程序写、`Uninstall` 键的 `DisplayName` 往往不是可执行文件、
//! `AppsFolder`（商店应用）要先 COM 一圈。而**开始菜单就是用户能点到的那个清单**：
//! 名字是他认识的、目标是他点过能开起来的、卸载时安装器会自己删掉。
//!
//! 代价：绿色软件（不建快捷方式）搜不到，商店应用（`AppsFolder`）搜不到。
//! 两者都要额外的数据源，等真的有人抱怨再说。
//!
//! ## 为什么不缓存
//!
//! 两个目录、递归读一遍 `lnk` 在本机是个位数毫秒。缓存要处理"什么时候失效"
//! （装完程序得重启才知道？），得不偿失——所以每次打开启动器现扫。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use zouwu_core::launcher::Program;

/// 开始菜单的根目录，**当前用户在前**：同名快捷方式优先选用户自己那份
/// （他装在自己目录里的版本，往往就是他想要的那个）。
fn roots() -> Vec<PathBuf> {
    const TAIL: &str = r"Microsoft\Windows\Start Menu\Programs";
    let mut out = Vec::new();
    for var in ["APPDATA", "PROGRAMDATA"] {
        if let Some(base) = std::env::var_os(var) {
            let path = Path::new(&base).join(TAIL);
            if path.is_dir() {
                out.push(path);
            }
        }
    }
    out
}

/// 递归深度上限。
///
/// `walk` 的起点是开始菜单（很浅），但用户可以把任意目录塞进 `[launcher] dirs`，
/// 有人会填 `C:\`。收紧到 6 层：正常的"程序/工具"目录不会更深，而滥用
/// 不至于把主线程卡住。
const MAX_DEPTH: usize = 6;

/// 递归收集 `dir` 下的快捷方式。
fn walk(dir: &Path, depth: usize, out: &mut Vec<Program>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            // 「启动」文件夹里的快捷方式是开机自动跑的东西，不是"我要打开的程序"。
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.eq_ignore_ascii_case("startup") || name == "启动" {
                continue;
            }
            walk(&path, depth + 1, out);
            continue;
        }
        if !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
        {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|n| n.to_str()) else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        out.push(Program::new(name, path.display().to_string()));
    }
}

/// 同名去重（保留先出现的那个），再按名字排好。
fn dedupe(programs: &mut Vec<Program>) {
    let mut seen = std::collections::HashSet::new();
    programs.retain(|program| seen.insert(program.name.to_lowercase()));
    programs.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.path.cmp(&b.path))
    });
}

/// 本机可搜到的全部程序（已去重、已排序）：开始菜单 + 用户在
/// `[launcher] dirs` 里指定的目录。
pub fn installed(extra: &[String]) -> Vec<Program> {
    let mut found = Vec::new();
    for root in roots() {
        walk(&root, 0, &mut found);
    }
    // 自定义目录排在后面去重：和开始菜单同名时留开始菜单那一份
    // （那是安装器装的、图标和参数都是对的）。
    let mut custom = Vec::new();
    for dir in extra {
        let dir = dir.trim();
        if dir.is_empty() {
            continue;
        }
        let path = Path::new(dir);
        if !path.is_dir() {
            log::warn!("launcher: extra dir `{dir}` is not a directory, skipped");
            continue;
        }
        walk(path, 0, &mut custom);
    }
    let custom_len = custom.len();
    found.extend(custom);
    dedupe(&mut found);
    log::info!(
        "launcher: {} programs found ({} shortcuts from {} extra dirs)",
        found.len(),
        custom_len,
        extra.len()
    );
    found
}

/// 打开一个快捷方式 / 程序。
///
/// **不能用 `launch::run`**：它走"是文件就直接 spawn"那条路，而 `.lnk` 不是
/// 可执行文件，`CreateProcess` 会失败。`start` 才会让 shell 去解快捷方式
/// （顺带处理 UAC、工作目录、`.url` 这些）。
pub fn open(path: &str) -> Result<(), String> {
    let raw = path.trim();
    if raw.is_empty() {
        return Err("empty path".to_string());
    }
    let mut cmd = Command::new(if cfg!(windows) { "cmd.exe" } else { "sh" });
    if cfg!(windows) {
        // `start` 的第一个参数是窗口标题，必须拿一个空串占位：
        // 否则带空格的路径会被它当成标题，然后什么都不打开。
        cmd.args(["/C", "start", "", raw]);
    } else {
        cmd.args(["-c", raw]);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    match cmd.spawn() {
        Ok(_) => {
            log::info!("launcher: opened `{raw}`");
            Ok(())
        }
        Err(err) => Err(format!("{raw}: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_finds_shortcuts_and_skips_startup_folder() {
        let root = std::env::temp_dir().join(format!("zouwu-programs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let nested = root.join("Tencent").join("WeChat");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(root.join("Startup")).unwrap();
        fs::write(nested.join("WeChat.lnk"), b"").unwrap();
        fs::write(root.join("Notepad.lnk"), b"").unwrap();
        fs::write(root.join("readme.txt"), b"").unwrap();
        fs::write(root.join("Startup").join("Auto.lnk"), b"").unwrap();

        let mut found = Vec::new();
        walk(&root, 0, &mut found);
        dedupe(&mut found);
        let names: Vec<&str> = found.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["Notepad", "WeChat"]);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn depth_limit_stops_runaway_trees() {
        let root = std::env::temp_dir().join(format!("zouwu-depth-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut deep = root.clone();
        for i in 0..(MAX_DEPTH + 3) {
            deep = deep.join(format!("d{i}"));
        }
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("TooDeep.lnk"), b"").unwrap();
        fs::write(root.join("Shallow.lnk"), b"").unwrap();

        let mut found = Vec::new();
        walk(&root, 0, &mut found);
        let names: Vec<&str> = found.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["Shallow"]);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn extra_dirs_are_scanned() {
        let root = std::env::temp_dir().join(format!("zouwu-extra-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("Tools")).unwrap();
        fs::write(root.join("Tools").join("Zouwu Probe.lnk"), b"").unwrap();

        let found = installed(&[
            root.display().to_string(),
            "  ".to_string(),
            r"Q:\nope".into(),
        ]);
        assert!(found.iter().any(|p| p.name == "Zouwu Probe"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dedupe_keeps_the_first_copy_and_sorts() {
        let mut programs = vec![
            Program::new("WeChat", r"C:\all\WeChat.lnk"),
            Program::new("notepad", r"C:\me\notepad.lnk"),
            Program::new("WeChat", r"C:\me\WeChat.lnk"),
        ];
        dedupe(&mut programs);
        assert_eq!(programs.len(), 2);
        assert_eq!(programs[0].name, "notepad");
        assert_eq!(programs[1].path, r"C:\all\WeChat.lnk");
    }
}
