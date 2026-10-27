//! 不用开界面就能验证/批量处理皮肤包。
//!
//! ```bash
//! cargo run -p zouwu-platform --example skinpack_tool -- list
//! cargo run -p zouwu-platform --example skinpack_tool -- export zouwu-ink /tmp/a.zwskin
//! cargo run -p zouwu-platform --example skinpack_tool -- import /tmp/a.zwskin
//! cargo run -p zouwu-platform --example skinpack_tool -- import --force /tmp/a.zwskin
//! ```
//!
//! 存在意义有两个：① 导入导出逻辑必须能脱离 GUI 验证（托盘菜单里点两下
//! 就算失败也不知道错在哪）；② 批量装机/脚本化分发时不用人点鼠标。

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(|s| s.as_str()) {
        Some("list") => {
            println!("config dir: {}", zouwu_platform::config_dir().display());
            let installed = zouwu_platform::assets::ensure_builtin_skins();
            if installed > 0 {
                println!("installed {installed} built-in skin(s)");
            }
            for id in zouwu_platform::assets::skin_ids() {
                match zouwu_platform::assets::load_skin(&id) {
                    Ok(pkg) => println!(
                        "  {id:<20} {} layer(s)  name={}  theme={}",
                        pkg.manifest.layers.len(),
                        pkg.manifest.display_name(),
                        pkg.manifest.theme.clone().unwrap_or_default()
                    ),
                    Err(err) => println!("  {id:<20} BROKEN: {err}"),
                }
            }
            0
        }
        Some("export") => match (args.get(1), args.get(2)) {
            (Some(id), Some(dest)) => {
                match zouwu_platform::skinpack::export_to(id, std::path::Path::new(dest)) {
                    Ok(()) => {
                        let size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
                        println!("exported `{id}` -> {dest} ({size} bytes)");
                        0
                    }
                    Err(err) => {
                        eprintln!("export failed: {err}");
                        1
                    }
                }
            }
            (Some(id), None) => match zouwu_platform::skinpack::export(id) {
                Ok(path) => {
                    println!("exported `{id}` -> {}", path.display());
                    0
                }
                Err(err) => {
                    eprintln!("export failed: {err}");
                    1
                }
            },
            _ => {
                eprintln!("usage: skinpack_tool export <skin-id> [dest.zwskin]");
                2
            }
        },
        Some("import") => {
            // `--force` = 面板里那个「覆盖已安装」勾选框的命令行版本
            let force = args.get(1).map(|s| s == "--force").unwrap_or(false);
            let path = if force { args.get(2) } else { args.get(1) };
            match path {
                Some(path) => {
                    match zouwu_platform::skinpack::import_with(std::path::Path::new(path), force) {
                        Ok(id) => {
                            println!("imported `{id}`");
                            0
                        }
                        Err(err) => {
                            eprintln!("import failed: {err}");
                            1
                        }
                    }
                }
                None => {
                    eprintln!("usage: skinpack_tool import [--force] <package.zwskin>");
                    2
                }
            }
        }
        _ => {
            eprintln!("usage: skinpack_tool <list|export|import> ...");
            2
        }
    };
    std::process::exit(code);
}
