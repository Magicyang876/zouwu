# Zouwu（驺吾）· 技术架构

> 本文是 [`README.md`](../README.md) 的技术展开：**这套代码是怎么拼起来的、为什么这么拼**。
> 只写已经存在的东西——计划中的功能和「为什么最终没这么做」，在 `decisions.md` 的 ADR 里。

---

## 1. 分层

```
┌──────────────────────────────────────────────────────────────────┐
│  zouwu-app        （bin）单实例、日志、托盘与窗口装配、主循环      │
├──────────────────────────────────────────────────────────────────┤
│  zouwu-ui         渲染 + 交互 + 三个真窗口                         │
│    app.rs         事件循环、状态机、热键派发、帧调度                │
│    frame.rs       开窗/摆位/上屏（先隐藏、摆好画好再显示）          │
│    paint.rs       Scene → tiny-skia 位图（兽、气泡、主题色）       │
│    widget.rs      弹簧动作、呼吸、眨眼                             │
│    skin.rs        皮肤解码（目录或 .zwskin → 图层 + Pixmap）       │
│    skin_menu.rs   右键形象菜单（真窗口，不是 TrackPopupMenu）      │
│    settings*.rs   设置面板（第二个窗，五页）                       │
│    history.rs     剪贴板历史窗      linefield.rs  单行编辑控件     │
│    present*.rs    呈现后端（分层窗口 / 实验性 wgpu）               │
│    text.rs icon.rs thumb.rs report.rs watchdog.rs                  │
├──────────────────────────────────────────────────────────────────┤
│  zouwu-core       领域层：**零 Win32，可以在任何平台跑测试**       │
│    geom anim ring config theme skin action hotkey history qr drag  │
├──────────────────────────────────────────────────────────────────┤
│  zouwu-platform   平台能力（每个都有 *_stub.rs 的占位实现）        │
│    layer 分层窗口  clipboard 剪贴板  hotkey 全局热键  tray 托盘    │
│    eyecare 护眼遮罩  altdrag Alt 拖动  screen 截屏  dialogs 对话框 │
│    text_gdi 文字  assets 内置素材  skinpack .zwskin  install 注册表│
│    logging 日志  single_instance 单实例  autostart 开机启动        │
├──────────────────────────────────────────────────────────────────┤
│  iconres          手写 COFF `.rsrc` 段（gnu 工具链下嵌 exe 图标）  │
└──────────────────────────────────────────────────────────────────┘
```

**依赖方向**：`app → ui → core`，`ui → platform`，`core` 谁都不依赖。
`core` 里的 `ring` / `anim` / `config` / `skin` 全是纯数据与纯函数，所以 273 个测试里
大部分不需要窗口、不需要显示器。

`iconres` 是个例外：它不依赖任何 crate，只负责把 `.ico` 拼成链接器认得的 COFF 目标文件。

---

## 2. 一帧的生命周期

```
winit 事件 ──► App::handle ──► 改状态（光标、弹簧、眨眼…）
                  │
                  ▼
         App::next_deadline()  ──►  Some(t) → ControlFlow::WaitUntil(t)
                  │                 None  → ControlFlow::Wait（真的一帧不画）
                  ▼
         paint::render(&Scene, &mut Pixmap)   // CPU 光栅，340×340
                  ▼
         Presenter::present(&pixmap)          // UpdateLayeredWindow
```

三条硬规则：

1. **重绘只有一个来源**：`next_deadline()`。没有定时器轮询、没有 `request_redraw` 满天飞。
   弹簧收敛、眨眼、呼吸这些"还会动的东西"才算 deadline，全部静止就是 `Wait`，
   空闲实测单核 0.3%。
2. **光标靠轮询，不靠消息**：分层窗口（`WS_EX_LAYERED`）拿不到可靠的
   `CursorMoved` / `MouseInput`，所以窗口内 16 ms、窗口外 100 ms 主动采样。
   代价是 `GetCursorPos` 永远在跑；收益是悬停、拖选、命中判定全部只有一条代码路径。
3. **窗口尺寸一次到位**：展开态要覆盖整个环，所以窗口一直是"最大",内部用 `SetWindowPos`
   一次性摆好（`layer::set_bounds`）。先 `request_inner_size` 再 `set_outer_position`
   会互相覆盖，屏幕上是"闪一下就回来"。

### 透明与命中

- `WS_EX_LAYERED` + `UpdateLayeredWindow`：逐像素 alpha，alpha = 0 的地方**天然鼠标穿透**，
  桌面图标照常可点——不需要维护命中遮罩位图。
- 面板窗口走 `frame::show()`：先建成隐藏窗口，摆好位置、画完第一帧再显示。
  否则能看到一帧"全尺寸透明窗"的闪烁。
- `always_on_top = false` 时，最大化的前台窗口会盖住挂件；此时注入点击前要用
  `WindowFromPoint` 确认最顶层是谁（`examples/` 里的截图脚本就是这么干的）。

---

## 3. 气泡环

```rust
// zouwu-core/src/ring.rs —— 纯几何，没有窗口
let ring = RingConfig::default();                 // 半径、气泡大小、间距
let rotation = solve_rotation(&ring, center, view, n);  // 贴边时把整环转开
let bubbles = ring.layout(n, center, rotation);   // 等角均分，第一颗在正上方
```

- **均匀分布**：每颗气泡占 `2π/n`，第一颗固定在正上方（配置里的第一项就是正上方）。
  方位不随数量变化——这是"肌肉记忆"，不是美学选择。
- **贴边自适应**：`solve_rotation` 只在环会被屏幕边缘切到时才转，转的角度是"刚好不切"。
- **展开/收起**：每颗气泡按索引错峰（`expand_stagger_ms`，默认 24 ms），缩放 + 位移 + 透明度；
  收起更快（60% 时长）。全部是弹簧，可被打断（从当前值出发，不跳变）。
- **一个球就是一条命令**：点球直接执行，没有二级菜单、没有文字牌
  （子项方案做过又删了，见 ADR-047）。环里只放真的能用的东西。

---

## 4. 皮肤与主题

### 4.1 皮肤 = 目录 + 数据

```
skins/zouwu-paper-2/
├── skin.toml      # 清单：id / name / geometry / 图层列表
├── body.png       # 主体
└── blink.png      # 闭眼（可选）
```

```toml
id = "zouwu-paper-2"
name = "折纸驺吾 2"

[geometry]
radius = 56.0          # 桌面上的主体半径（逻辑像素）
breath_amount = 0.03   # 呼吸缩放幅度

[[layers]]
image = "body.png"
scale = 1.67           # 长边 = 2 × radius × scale

[[layers]]
image = "blink.png"
role = "blink"         # body / blink / overlay
scale = 1.67
```

- **`scale` 是唯一的尺寸口径**：图层按"长边 = 2 × 实际半径 × scale"等比缩放，
  所以同一套 PNG 在任何 DPI 下都不用改。
- **`.zwskin` 是同一个目录的打包**（zip + 严格校验：id 不能含路径分隔符、图层必须
  PNG、scale 在 0.05..8）。导入导出在托盘里。
- **内置皮肤是内嵌的**：发布版是单个 exe，没有 `assets/` 目录，所以内置皮肤的字节
  `include_bytes!` 进程序，**首次启动落到 `%APPDATA%\zouwu\skins\`**，之后以磁盘为准
  （用户改我们发布的皮肤不会被覆盖）。改了内置字节必须把
  `BUILTIN_ASSET_REVISION` +1，否则老安装上那份永远不换。
- **没有图片 = 代码绘制**：`procedural` 那只兽是 `paint.rs` 里用路径画的。

皮肤图全部由 `crates/zouwu-ui/examples/make_*.rs` 生成（不手画 PNG），
每个生成器都带自检：质心必须落在画布中心附近、外轮廓不能超过气泡环、
可见像素数不能塌。

### 4.2 主题 = TOML 增量覆盖

内置 `zouwu-warm` / `zouwu-night`，用户主题放 `%APPDATA%\zouwu\themes\`，
只写要改的令牌，其余继承。令牌就是一组颜色与圆角/描边粗细，`paint.rs` 只认令牌，
不认具体色值——所以加一套配色不用改代码。

---

## 5. 配置与持久化

```
%APPDATA%\zouwu\
├── config.toml      # 唯一真相；设置面板没有"保存"按钮，改完立刻写
├── logs\zouwu.log   # 一行一句、带模块名；滚到 2 MB 留 3 份
├── logs\watchdog.log# 主线程卡死时唯一还能写的日志（不分配内存）
├── history.toml     # 剪贴板历史
├── skins\ themes\ icons\
```

- `SCHEMA_VERSION = 9`，读旧配置时逐版迁移；认不出的字段忽略而不是报错。
- 热重载：外部改了 `config.toml` → 重新应用热键、主题、护眼强度，不需要重启。
- 单实例：第二个进程用命名事件找到已有实例，把窗口叫出来，然后自己退出。

---

## 6. 性能预算（release，340² 画布）

| 项 | 实测 |
|---|---|
| 单帧光栅（收起） | ~490 µs |
| 单帧光栅（展开 + 7 颗气泡） | ~1129 µs |
| `UpdateLayeredWindow` | 350–430 µs |
| 空闲 CPU | 单核 0.31%（60 s 采样） |
| 常驻帧率 | 3.4–5 fps（只在眨眼/呼吸时） |
| 内存 RSS | ~17 MB（无字体）／23 MB（加载中文字体后） |

量法：`cargo run --release -p zouwu-ui --example bench`。诊断帧率用 `ZOUWU_PERF=1`。

---

## 7. 可测试性

- `cargo test --workspace` → **273 passed**。绝大多数在 `core`（几何、环布局、配置迁移、
  皮肤清单解析）与 `ui`（绘制像素数、面板布局、输入状态机）里，不需要显示器。
- **离屏渲染**是主要验证手段：`examples/offscreen.rs` 复刻真实布局出一堆 PNG
  （收起 / 展开 / 深色 / 形象菜单 / 悬停高亮），`examples/settings_shot.rs` 出设置面板，
  `examples/history_shot.rs` 出剪贴板历史。改完看 PNG，比反复截图快得多。
- 需要真实窗口的验证（DPI、Z 序、焦点、拖选）才用注入鼠标键盘——
  那些脚本不进仓库，因为它们的坑（125% 缩放下要 `SetProcessDpiAwarenessContext`、
  `SendKeys` 会卡死）比代码本身长。

---

## 8. 平台能力清单

真的实现了的：托盘菜单、全局热键、开机自启、剪贴板读写与历史、护眼遮罩
（每显示器一块 `WS_EX_TRANSPARENT` 遮罩）、Alt 拖动任意窗口、截屏、
二维码解码（纯 Rust 实现在 `core::qr`，图从截屏来）、
设置/形象/剪贴板历史三个窗、`.zwskin` 导入导出、`--install` / `--uninstall`
（写注册表与开始菜单）。

**没做**：截图标注、翻译、OCR、取色、番茄钟、脚本插件、自动更新。
README 的「Not there yet」列着同一份清单。

---

## 9. 已知限制

- 只支持 Windows 10/11 x64。`window` 相关的平台代码都在 `zouwu-platform` 里，
  每个模块都有 `*_stub.rs`，换平台要重写的只有这一层——但它确实还没写。
- 挂件是 `WS_EX_LAYERED` 分层窗口，**没有 GPU 加速**：340×340 的 CPU 光栅在这个尺寸
  下比任何 GPU 路径都省，代价是画不了全屏特效。
- 中文文字走 GDI（`text_gdi`），字体按语言选；没有做完整的字体回退链。
- 全局热键被别的程序占走时只记日志、不弹窗（`hotkey` 注册失败是常态，弹窗更烦）。