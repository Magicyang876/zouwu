# 驺吾 Zouwu

> [English](README.md) | **简体中文**

一只常驻桌面角落的小兽。鼠标移上去，它会「变身」张开一圈气泡——扫二维码、
翻剪贴板历史，或者你随手挂上去的任何命令。空闲时它只花 **单核的 0.3%、约 17 MB 内存**，
因为这里没有 webview，也不用 GPU：只有一个用 CPU 光栅器画出来的 Win32 分层窗口。

<p align="center">
  <img src="assets/screenshots/preview.gif" width="340" alt="鼠标移上去，气泡环依次弹开">
</p>

<p align="center">
  <img src="assets/screenshots/ring-light.png" width="300" alt="驺吾展开气泡环（浅色主题）">
  <img src="assets/screenshots/ring-dark.png" width="300" alt="驺吾展开气泡环（深色主题）">
</p>

<p align="center">
  <a href="https://github.com/magicyang876/zouwu/releases"><img alt="Platform" src="https://img.shields.io/badge/platform-Windows%2010%2F11-0078D4"></a>
  <img alt="Rust" src="https://img.shields.io/badge/rust-1.80%2B-dea584">
  <img alt="License" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue">
  <img alt="No webview" src="https://img.shields.io/badge/webview-none-success">
</p>

驺吾出自《山海经·海内北经》：「林氏国有珍兽，大若虎，五采毕具，尾长于身，名曰驺吾，
乘之日行千里。」这个程序借的是它的形与名——一只五彩的小兽蹲在桌面上，离你每天重复
一百遍的那些操作，只有一次悬停的距离。

---

## 功能

下面这些都已经接上线、现在就能用。环上不会出现「点了只写一行日志」的装饰性气泡
——这是刻意的清理。

**本体**

| | |
|---|---|
| 置顶分层窗口 | 无边框、透明、逐像素 alpha、不进任务栏；小兽之外的地方点击穿透 |
| 眨眼与呼吸 | deadline 调度的空闲动画；**没有变化时一帧都不画** |
| 气泡环 | 悬停 150 ms（或点一下）弹出 N 颗气泡：弹簧曲线、24 ms 错峰、缓慢轨道浮动 |
| 拖动与记忆 | 拖着走；位置跨重启保留；显示器变化导致它落在屏幕外时，会自己走回来 |
| 托盘 / 热键 / 单实例 | `Alt+Space` 呼出；关窗只是隐藏；再启动一次会把已有实例叫出来 |

**内置功能**

- **二维码识别**（`Ctrl+Alt+Q`）：截整块虚拟桌面（多屏），扫出所有码，用独立窗口
  列出结果，点一行即复制。
- **剪贴板历史**（`Ctrl+Alt+V`）：只存文本，落盘保存，同内容不重复占行，上限 100 条。
  输入即搜索（空格分隔多关键词取「与」，支持中文输入法），
  `↑`/`↓`/`PgUp`/`PgDn`/滚轮翻页，`Enter` 粘贴，`Delete` 删除。
- **自定义命令球**：环上可以挂「运行程序 / 执行命令」的球，每颗球单独绑全局热键，
  顺序和开关都归你。

**桌面顺手的事**

- **护眼**：整块虚拟桌面盖一层琥珀色遮罩，点穿透、不抢焦点，新出现的置顶窗口会被
  毫秒级重新盖上。
- **Alt 拖动窗口**：按住 `Alt` 拖任意窗口；`Alt`+双击最大化/还原；`Alt`+滚轮上滚最大化、
  下滚最小化。可以在设置里整个关掉。

**外观与语言**

- 主题是 TOML 增量覆盖（内置暖橙 / 夜行两套）；皮肤是 PNG 图层目录；`.zwskin`
  单文件皮肤包带严格校验，托盘里可导入导出。
- 中文 / 英文（Fluent），运行时切换无需重启；`language = "auto"` 跟随系统。

### 还没做的

截图与标注、翻译、OCR、取色、番茄钟、文本启动器、脚本插件、自动更新都**只是计划，
尚未实现**。环上只放真的能用的东西。

---

## 安装

从 [Releases](https://github.com/magicyang876/zouwu/releases) 下载
`zouwu-<version>-win64.zip`（约 1.9 MB），解压，双击 `zouwu.exe`——就这样。

```powershell
# 想要开始菜单入口和「设置 → 应用」里的卸载项：
.\zouwu.exe --install

# 卸载（保留配置）
.\zouwu.exe --uninstall

# 卸载并清空配置 / 皮肤 / 日志
.\zouwu.exe --uninstall --purge
```

没有 MSI、没有安装向导、不需要管理员权限：便携 zip 加上 `--install` 已经覆盖了安装器
能做的三件事，而少一个安装器就少一份要签名的东西。

**需要** Windows 10/11 x64。发布包**没有代码签名**，SmartScreen 可能提示「未知发布者」——
请对照发布页上的 SHA-256。

数据都在这里：

```
%APPDATA%\zouwu\config.toml     配置（保存即生效，不用重启）
%APPDATA%\zouwu\skins\          皮肤（导入的 .zwskin 解到这里）
%APPDATA%\zouwu\themes\         自定义配色
%APPDATA%\zouwu\history.toml    剪贴板历史
%APPDATA%\zouwu\logs\zouwu.log  日志（2 MB × 3 轮转）——出问题先看它
```

---

## 从源码构建

工具链固定在 `stable-x86_64-pc-windows-gnu`（见 `rust-toolchain.toml`），链接器用
`rust-lld`，因此**不需要装 Visual Studio**。MSVC 也能编，区别只在图标资源怎么进 exe。

```bash
cargo run --release -p zouwu-app        # 跑起来
cargo test --workspace                  # 271 个单测，不依赖任何外部夹具
cargo fmt --all --check
```

改界面时最省事的三条（都不需要桌面环境，直接渲染成 PNG）：

```bash
cargo run -p zouwu-ui --example offscreen        # 常态 / 展开 / 高 DPI 三张图
cargo run -p zouwu-ui --example settings_shot    # 设置面板所有分页，1x 与 2x
cargo run --release -p zouwu-ui --example bench  # 单帧成本模型
ZOUWU_PERF=1 cargo run --release -p zouwu-app    # 每 60 帧报一次实际帧率
```

打包发布就一个 zip——一个 exe 就是全部：

```bash
cargo build --release -p zouwu-app        # -> target/release/zouwu.exe
```

---

## 配置

全部在 `%APPDATA%\zouwu\config.toml`，保存即热重载；配置文件坏了只会跳过并保留现状，
不会把当前状态搞乱。大致长这样：

```toml
schema_version = 9

[general]
language = "auto"          # "zh-CN" | "en-US" | "auto"
hotkey = "Alt+Space"
start_on_boot = false

[appearance]
theme = "zouwu-warm"
skin = "zouwu-ink"

[eye_care]
enabled = false
strength = 35              # 0..100

[alt_drag]
enabled = true

[clipboard]
enabled = true
limit = 100

[interaction]
hover_delay_ms = 150
collapse_delay_ms = 250
expand_on_summon = false

[ring]
radius = 112.0             # 气泡环离主体的距离
bubble_radius = 24.0
max_per_ring = 8
orbit_start_deg = -90.0

# 一条 = 一颗球，列表顺序就是展开顺序
[[ring.items]]
id = "builtin.qrcode"
enabled = true
hotkey = "Ctrl+Alt+Q"

[[ring.items]]
id = "custom.1"
enabled = true
hotkey = "Ctrl+Alt+E"
label = "编辑器"
command = "notepad.exe"
```

这些大部分也有界面：设置面板是第二个真窗口（常规 / 交互 / 外观 / 形象 / 关于 五页），
**没有保存按钮**——改动即时生效并写盘。

---

## 它是怎么做的

```
crates/zouwu-core      领域层：几何、弹簧动画、环布局、配置 schema、主题、皮肤、
                       i18n、剪贴板模型、二维码胶水
crates/zouwu-platform  Win32：分层窗口、托盘、全局热键、剪贴板、开机自启、GDI 文本、
                       单实例、Alt 拖动钩子、安装、轮转日志
crates/zouwu-ui        光栅化（tiny-skia）、场景绘制、设置面板、剪贴板 / 二维码 /
                       形象选择窗口
crates/zouwu-app       组装、主循环、命令行（--install / --uninstall / --version）
crates/iconres         构建期生成 COFF 资源对象，让 exe 自己带图标
```

四条决策解释了大部分代码长什么样：

1. **不要 webview，也不要 GPU。** 透明、点击穿透、不抢焦点、逐像素 alpha 的窗口，
   正好是 `UpdateLayeredWindow` + CPU 光栅最擅长、而 webview 最吃亏的地方。
   release exe 约 7 MB，用户不用装任何运行时。
2. **事件驱动的帧。** 按 deadline 调度并自适应帧率：呼吸时 20 fps，交互时 60 fps，
   **空闲时 0 帧**。实测空闲占单核 0.31%、3.4~5 fps、RSS 约 17.7 MB；隐藏到托盘后
   的 15 秒内 CPU 为 0 ms。
3. **用系统字体栈，不引字体库。** 进程内解析 CJK 字体让面板 RSS 冲到 879 MB，
   改用 GDI 栅格化后是 23 MB。
4. **轮询光标，不信命中测试。** 分层窗口收不到可靠的鼠标消息，所以按 16 ms（窗口内）/
   100 ms（窗口外）的节拍主动采样。

---

## 参与

欢迎提 issue 和 PR。提交前请跑：

```bash
cargo test --workspace && cargo fmt --all --check
```

如果改动影响了行为，请在提交信息或 PR 描述里写清「为什么」——那个理由才是以后要看的，
「为什么」比「是什么」更耐放。平台相关的代码一律放在 `zouwu-platform` 的 trait 后面，
`zouwu-core` 与 `zouwu-ui` 必须保持平台无关，这样将来移植 macOS / Linux 才有路。

## 许可

代码采用 [MIT](LICENSE-MIT) 或 [Apache-2.0](LICENSE-APACHE) 双许可，任选其一。
`assets/` 下的美术资源与皮肤为 CC BY 4.0，另有说明者除外。