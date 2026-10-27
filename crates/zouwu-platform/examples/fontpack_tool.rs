//! 字体包的命令行入口：`list` / `import <文件>` / `delete <id>`。
//!
//! 设置面板里能做的事这里都能做，但命令行有它的用处：批量导入、
//! 以及在没法点鼠标的环境（远程会话、脚本）里验证字体到底装上了没有。

use std::path::PathBuf;

fn main() {
    zouwu_platform::logging::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("list") | None => {
            let packs = zouwu_platform::fontpack::list();
            if packs.is_empty() {
                println!(
                    "(no font packs; fonts dir: {})",
                    zouwu_platform::fontpack::fonts_dir().display()
                );
            }
            for pack in packs {
                println!(
                    "{}  family={}  files={:?}",
                    pack.id, pack.family, pack.files
                );
            }
            0
        }
        Some("import") => match args.get(1) {
            Some(path) => match zouwu_platform::fontpack::import(&PathBuf::from(path)) {
                Ok(pack) => {
                    println!("imported: {} ({})", pack.id, pack.family);
                    0
                }
                Err(err) => {
                    eprintln!("import failed: {err}");
                    1
                }
            },
            None => {
                eprintln!("usage: fontpack_tool import <file.ttf>");
                2
            }
        },
        Some("delete") => match args.get(1) {
            Some(id) => match zouwu_platform::fontpack::delete(id) {
                Ok(()) => {
                    println!("deleted: {id}");
                    0
                }
                Err(err) => {
                    eprintln!("delete failed: {err}");
                    1
                }
            },
            None => {
                eprintln!("usage: fontpack_tool delete <id>");
                2
            }
        },
        Some(other) => {
            eprintln!("unknown command `{other}`; expected list | import | delete");
            2
        }
    };
    std::process::exit(code);
}
