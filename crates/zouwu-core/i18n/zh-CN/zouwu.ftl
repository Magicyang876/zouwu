# 简体中文（默认）
# 注意：Fluent 的 id 不允许出现 "."，代码里会把 "action.screenshot" 之类
# 的键名里的点替换成连字符后再查找，见 `zouwu-core::i18n::key_to_id`。

app-name = 驺吾
app-tagline = 桌面上的山海经小兽

tray-show = 显示驺吾
tray-hide = 隐藏驺吾
tray-config = 配置文件…
tray-language = 语言
tray-language-auto = 跟随系统
tray-theme = 主题
tray-skin = 形象
tray-skin-import = 导入皮肤包…
tray-skin-export = 导出当前皮肤…
tray-recenter = 回到主屏
tray-autostart = 开机自启
tray-quit = 退出

action-screenshot = 截图
action-translate = 翻译
action-qrcode = 扫码
action-clipboard = 剪贴板
action-colorpicker = 取色
action-launcher = 启动器
action-pomodoro = 番茄钟
action-more = 更多

hint-hover = 鼠标停在驺吾身上可以展开
hint-hotkey = 按 {$hotkey} 可以随时呼出
# ── 设置面板 ──────────────────────────────────────────────
tray-settings = 设置…
tray-open-logs = 打开日志文件夹…

settings-title = 驺吾 · 设置
settings-auto-saved = 改动即时生效，无需保存
settings-tab-general = 常规
settings-tab-interaction = 交互
settings-tab-appearance = 外观
settings-tab-about = 关于

settings-language = 界面语言
settings-language-auto = 跟随系统
settings-hotkey = 呼出热键
settings-hotkey-hint = 点击后按下组合键；Esc 取消，Backspace 清除
settings-hotkey-capturing = 正在捕获按键…
settings-hotkey-empty = 未设置
settings-start-on-boot = 开机自启
settings-show-on-start = 启动时显示
settings-bring-to-front = 呼出时置于最前

settings-hover-delay = 悬停展开延迟
settings-hover-factor = 悬停热区（小兽半径的倍数）
settings-collapse-factor = 收起距离（环半径的倍数）
settings-expand-on-click = 点击立即展开
settings-hit-tolerance = 气泡命中容差
settings-buddy-radius = 小兽大小
settings-breathe = 呼吸动画

settings-theme = 主题
settings-skin = 皮肤
settings-skin-procedural = 代码绘制（内置）
settings-ring-radius = 气泡环半径
settings-bubble-radius = 气泡大小
settings-float-amplitude = 轨道浮动幅度
settings-expand-stagger = 气泡出场错峰

settings-log-level = 日志级别
settings-log-to-file = 写入日志文件
settings-open-logs = 打开日志文件夹
settings-open-config = 打开配置文件夹
settings-reset = 恢复默认设置
settings-reset-hint = 只恢复外观与交互，不动语言和自启

settings-about-version = 版本
settings-about-platform = 平台
settings-about-config = 配置文件
settings-about-log = 日志文件
settings-ms = {$ms} 毫秒
settings-px = {$px} 像素

# ── 设置面板（M1 新增） ────────────────────────────────────
settings-window-size = 窗口大小
settings-always-on-top = 窗口置顶
settings-collapse-delay = 收起延迟
settings-section-behavior = 行为
settings-section-beast = 小兽
settings-section-ring = 气泡环
settings-section-logging = 日志
settings-log-file = 日志文件
settings-footer-keys = ↑↓ 选择 · ←→ 调整 · Esc 关闭
settings-reset-done = 已恢复默认外观与交互
settings-hotkey-need-modifier = 需要至少一个修饰键（Ctrl / Alt / Shift / Win）

skin-import-failed = 皮肤包导入失败
skin-export-procedural = 默认形象是代码画出来的，没有可导出的文件。想分享，先换成一套素材皮肤。
skin-export-failed = 皮肤包导出失败

# ── 形象页（设置面板） ─────────────────────────────
settings-tab-skins = 形象
settings-section-skins = 形象
settings-section-skinpack = 皮肤包
settings-skin-current = 当前形象
settings-skin-gallery-hint = 点缩略图直接切换，选中的会立刻换到桌面
settings-skin-import = 导入皮肤包…
settings-skin-export = 导出当前形象…
settings-skin-open-dir = 打开形象目录
settings-skin-delete = 删除这套形象
settings-skin-overwrite = 覆盖已安装的同名皮肤
settings-skin-overwrite-hint = 导入同名皮肤时直接替换，而不是拒绝
settings-skin-overwrite-more = 详细说明…
settings-skin-overwrite-less = 收起说明
settings-skin-overwrite-note = 皮肤包靠内部的名字（id）区分。导入一套与已安装皮肤同名的包时：勾上就整套替换 —— 原来的图层、配色、几何参数全部丢掉，改成新包里的，导入前会弹一次确认；不勾则直接拒绝导入，保留已经装好的那套。
  想保留旧皮肤又想装新的，可以先把当前这套导出备份，或者改名后再导入。这个勾选只影响"这一次导入"，不会记住。
settings-skin-overwrite-confirm = 覆盖已安装的皮肤？
settings-skin-delete-hint = 内置形象删掉后，下次启动会自动补回来
settings-skin-delete-failed = 删除失败
settings-skin-imported = 已导入「{$id}」
settings-skin-broken = 打不开（清单或图片有问题）

skin-import-overwrite-hint = 如果这是同一个皮肤的新版本，在「形象」页勾上「覆盖已安装的同名皮肤」再导入。

settings-about-install = 安装位置
settings-about-install-portable = 便携运行（未安装）
settings-about-install-other = 已安装到另一个位置

# ── 字体包 ────────────────────────────────────────────────
tray-font-dir = 字体目录…
settings-font = 字体
settings-font-hint = 导入 .ttf / .otf / .ttc，整个面板立刻换成该字体
settings-font-family = 家族名
settings-font-family-system = 系统默认字体
settings-font-system = 系统默认
settings-font-import = 导入字体…
settings-font-import-failed = 字体导入失败
settings-font-open-dir = 打开字体目录
settings-font-delete = 删除字体…
settings-font-delete-hint = 删掉后立即回到系统默认字体
settings-font-delete-confirm = 确定删除字体包“{$name}”吗？字体文件会被删掉，无法撤销。
settings-font-delete-failed = 字体删除失败
settings-font-no-cjk = 该字体没有中文字形：中文会退回系统字体（宽度仍按它算，可能显得偏窄）
