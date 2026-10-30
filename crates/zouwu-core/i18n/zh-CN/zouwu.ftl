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
settings-tab-bubbles = 气泡球
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
settings-expand-on-summon = 呼出即展开气泡环
settings-expand-on-summon-hint = 呼出后一直摆着，点小兽或右键才收起
settings-hit-tolerance = 气泡命中容差
settings-buddy-radius = 小兽大小
settings-breathe = 呼吸动画

settings-theme = 主题
settings-skin = 皮肤
settings-skin-procedural = 代码绘制（内置）
settings-ring-radius = 气泡环离主体的距离
settings-orbit-start = 第一颗球的角度
settings-bubble-radius = 气泡大小
settings-max-per-ring = 一圈最多几颗
settings-ring-gap = 外圈间距
settings-orbit-start-hint = 0° 正右，-90° 正上方，180° 正左
settings-ring-gap-hint = 第二圈比第一圈再远多少
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
settings-bubbles-count = {$count} 颗

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

tray-scan-qr = 扫描二维码
qr-title = 二维码
qr-window-title = 二维码扫描结果
qr-found = 识别到 {$count} 个二维码
qr-none = 没有找到二维码
qr-none-detail = 已扫描 {$width}×{$height} 的整个桌面
qr-error = 扫描失败
qr-unreadable = 有 {$count} 个码读不出来
qr-elapsed = 用时 {$ms} 毫秒
qr-screen = 显示器 {$index}
qr-footer = 点一行复制内容 · Esc 关闭
qr-copied = 已复制到剪贴板

# ── 护眼（蓝光过滤） ────────────────────────────────────
settings-section-eye-care = 护眼
settings-eye-care = 过滤蓝光
settings-eye-care-hint = 整个屏幕加一层暖色，蓝色少了，长时间看没那么刺眼
settings-eye-care-strength = 过滤强度
settings-eye-care-strength-hint = 越大越暖；开到最大约 55% 的暖色遮罩

# ── 窗口拖动（AltSnap 那一套） ─────────────────
settings-section-alt-drag = 窗口拖动
settings-alt-drag = 按住 Alt 拖动窗口
settings-alt-drag-hint = 按住 Alt 拖左键可移动光标下的窗口；双击最大化（再双击还原）；滚轮上滚最大化、下滚最小化

# ── 剪贴板历史 ────────────────────────────────────────────
settings-count = {$count} 条
settings-section-clipboard = 剪贴板
settings-clipboard = 记录剪贴板历史
settings-clipboard-hint = 复制过的文本会自动留下，重启后仍在
settings-clipboard-limit = 最多保留
settings-clipboard-limit-hint = 超出后先丢最旧的；同样的内容不会重复占一行
settings-clipboard-clear = 清空历史
settings-clipboard-clear-hint = 只删记录，不动系统剪贴板里的东西
settings-clipboard-clear-button = 清空…
settings-clipboard-clear-confirm = 确定清空全部剪贴板历史吗？这一步无法撤销。
tray-clipboard = 剪贴板历史
skin-menu-title = 形象

history-title = 剪贴板历史
history-footer = ↑↓ 选择 · Enter 复制并关闭 · 输入即搜索 · Delete 删除 · Esc 关闭
history-footer-empty = 还没有记录，复制点什么就会出现在这里
history-empty = 还没有记录
history-search = 搜索（空格分隔多个关键词）
history-no-match = 没有匹配的条目

# ── 气泡球（环上的功能清单） ──────────────────────────────
settings-section-bubbles = 顺序与热键
settings-bubbles-note = 排在前面先展开；关掉的球不上环，环上按启用数量自动均分。每个球可以单独绑热键，留空就是没有。
settings-bubbles-name = 名称
settings-bubbles-name-hint = 显示在设置里；环上只有图标
settings-bubbles-command = 命令
settings-bubbles-command-hint = 一行命令行，例如 notepad.exe 或 code C:\work
settings-bubbles-icon = 图标
settings-bubbles-icon-default = 默认图标
settings-bubbles-icon-pick = 更换图标…
settings-bubbles-icon-failed = 图标导入失败
settings-bubbles-remove = 移除这颗球
settings-bubbles-remove-hint = 移除后不可撤销
settings-section-bubbles-add = 新增
settings-bubbles-add-command = 新增一颗命令球
settings-bubbles-add-command-hint = 先加一颗空的，再给它填名称和命令
settings-bubbles-add-program = 从程序新增…
settings-bubbles-empty = 一颗球都没有
settings-bubbles-empty-text = 未填写
settings-bubbles-full = 已经到上限了（最多 { $max } 颗）
settings-bubbles-hotkey-taken = 这个热键已经给了「{ $name }」
settings-hotkey-none = 无热键

# ── 自定义球：命令拉不起来时的结果窗 ──
report-launch-failed = 这个球没跑起来
report-launch-footer = 可以在设置 › 气泡球 里改命令或换图标
