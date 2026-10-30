# English (fallback locale)
# NOTE: Fluent ids cannot contain ".". Internal keys like "action.screenshot"
# are normalised to "action-screenshot" before lookup; see `key_to_id`.

app-name = Zouwu
app-tagline = A little beast from the Classic of Mountains and Seas

tray-show = Show Zouwu
tray-hide = Hide Zouwu
tray-config = Config folder…
tray-language = Language
tray-language-auto = Follow system
tray-theme = Theme
tray-skin = Skin
tray-skin-import = Import skin package…
tray-skin-export = Export current skin…
tray-recenter = Bring back to screen
tray-autostart = Start with Windows
tray-quit = Quit

action-screenshot = Screenshot
action-translate = Translate
action-qrcode = Scan QR
action-clipboard = Clipboard
action-colorpicker = Pick color
action-launcher = Launcher
action-pomodoro = Pomodoro
action-more = More

hint-hover = Hover the beast to unfold
hint-hotkey = Press {$hotkey} to summon it anytime
# ── Settings panel ───────────────────────────────────────
tray-settings = Settings…
tray-open-logs = Open log folder…

settings-title = Zouwu · Settings
settings-auto-saved = Changes apply instantly — nothing to save
settings-tab-general = General
settings-tab-interaction = Interaction
settings-tab-appearance = Appearance
settings-tab-bubbles = Bubbles
settings-tab-about = About

settings-language = Language
settings-language-auto = Follow system
settings-hotkey = Summon hotkey
settings-hotkey-hint = Click, then press a combo; Esc cancels, Backspace clears
settings-hotkey-capturing = Waiting for keys…
settings-hotkey-empty = Not set
settings-start-on-boot = Start with Windows
settings-show-on-start = Show on launch
settings-bring-to-front = Bring to front on summon

settings-hover-delay = Hover delay before unfolding
settings-hover-factor = Hover hot zone (× body radius)
settings-collapse-factor = Collapse distance (× ring radius)
settings-expand-on-click = Click unfolds immediately
settings-expand-on-summon = Unfold the ring on summon
settings-expand-on-summon-hint = Stays open until you click the beast or right-click
settings-hit-tolerance = Bubble hit tolerance
settings-buddy-radius = Beast size
settings-breathe = Breathing animation

settings-theme = Theme
settings-skin = Skin
settings-skin-procedural = Procedural (built in)
settings-ring-radius = Bubble ring radius
settings-bubble-radius = Bubble size
settings-float-amplitude = Orbit drift amplitude
settings-expand-stagger = Bubble stagger

settings-log-level = Log level
settings-log-to-file = Write log file
settings-open-logs = Open log folder
settings-open-config = Open config folder
settings-reset = Reset to defaults
settings-reset-hint = Resets appearance and interaction only — language and autostart stay

settings-about-version = Version
settings-about-platform = Platform
settings-about-config = Config file
settings-about-log = Log file
settings-ms = {$ms} ms
settings-px = {$px} px

# ── Settings panel (added in M1) ──────────────────────────
settings-window-size = Window size
settings-always-on-top = Always on top
settings-collapse-delay = Collapse delay
settings-section-behavior = Behaviour
settings-section-beast = The beast
settings-section-ring = Bubble ring
settings-section-logging = Logging
settings-log-file = Log file
settings-footer-keys = ↑↓ select · ←→ adjust · Esc close
settings-reset-done = Appearance and interaction reset to defaults
settings-hotkey-need-modifier = Needs at least one modifier (Ctrl / Alt / Shift / Win)

skin-import-failed = Skin package import failed
skin-export-procedural = The default character is drawn in code, so there are no files to export. Switch to an image-based skin first.
skin-export-failed = Skin package export failed

# ── Skins (settings panel) ─────────────────────────
settings-tab-skins = Skins
settings-section-skins = Character
settings-section-skinpack = Skin packages
settings-skin-current = Current skin
settings-skin-gallery-hint = Click a thumbnail to switch — it applies immediately
settings-skin-import = Import skin package…
settings-skin-export = Export current skin…
settings-skin-open-dir = Open skins folder
settings-skin-delete = Delete this skin
settings-skin-overwrite = Overwrite the installed skin with the same id
settings-skin-overwrite-hint = Replace a skin with the same id instead of refusing it
settings-skin-overwrite-more = What does this do?
settings-skin-overwrite-less = Hide the explanation
settings-skin-overwrite-note = A skin package is identified by the name (id) inside it. When you import one whose id is already installed: checked, the whole skin is replaced — its layers, palette and geometry are dropped in favour of the new package, after one confirmation; unchecked, the import is refused and the installed copy stays as it is. To keep both, export the current skin first or rename the incoming one. This checkbox only applies to the next import; it is not remembered.
  settings-skin-overwrite-confirm = Overwrite the installed skin?
settings-skin-delete-hint = Built-in skins come back on the next launch
settings-skin-delete-failed = Delete failed
settings-skin-imported = Imported "{$id}"
settings-skin-broken = cannot be opened (bad manifest or image)

skin-import-overwrite-hint = If this is a newer version of the same skin, tick "Overwrite the installed skin with the same id" on the Skins tab and import again.

settings-about-install = Installed at
settings-about-install-portable = Portable (not installed)
settings-about-install-other = Installed elsewhere

# ── font packs ────────────────────────────────────────────
tray-font-dir = Fonts folder…
settings-font = Font
settings-font-hint = Import .ttf / .otf / .ttc and the whole panel switches at once
settings-font-family = Family
settings-font-family-system = System default
settings-font-system = System default
settings-font-import = Import font…
settings-font-import-failed = Could not import the font
settings-font-open-dir = Open fonts folder
settings-font-delete = Delete font…
settings-font-delete-hint = Deleting falls back to the system font right away
settings-font-delete-confirm = Delete the font pack "{$name}"? The files are removed and this cannot be undone.
settings-font-delete-failed = Could not delete the font
settings-font-no-cjk = This font has no Chinese glyphs: Chinese text falls back to the system font (but keeps this font's widths, so it may look squeezed)

tray-scan-qr = Scan QR code
qr-title = QR code
qr-window-title = QR scan results
qr-found = Found {$count} code(s)
qr-none = No QR code found
qr-none-detail = Scanned the whole desktop ({$width}×{$height})
qr-error = Scan failed
qr-unreadable = {$count} code(s) could not be read
qr-elapsed = took {$ms} ms
qr-screen = display {$index}
qr-footer = Click a row to copy · Esc to close
qr-copied = Copied to clipboard

# ── Eye care (blue light filter) ─────────────────────────
settings-section-eye-care = Eye care
settings-eye-care = Filter blue light
settings-eye-care-hint = Adds a warm layer over the whole screen; less blue is easier on the eyes
settings-eye-care-strength = Filter strength
settings-eye-care-strength-hint = Higher is warmer; the maximum is roughly a 55% warm overlay

# ── Window drag (AltSnap style) ─────────────────────────
settings-section-alt-drag = Window drag
settings-alt-drag = Hold Alt to drag windows
settings-alt-drag-hint = Hold Alt and drag with the left button to move the window under the cursor; double-click to maximize (again to restore); wheel up maximizes, wheel down minimizes

# ── Clipboard history ─────────────────────────────────────
settings-count = {$count} items
settings-section-clipboard = Clipboard
settings-clipboard = Keep clipboard history
settings-clipboard-hint = Copied text is remembered across restarts
settings-clipboard-limit = Keep at most
settings-clipboard-limit-hint = The oldest entries go first; repeats never take a second row
settings-clipboard-clear = Clear history
settings-clipboard-clear-hint = Only deletes the record; the system clipboard is untouched
settings-clipboard-clear-button = Clear...
settings-clipboard-clear-confirm = Clear the whole clipboard history? This cannot be undone.
tray-clipboard = Clipboard history
history-title = Clipboard history
history-footer = ↑↓ select · Enter copy and close · Delete remove · Esc close
history-footer-empty = Nothing yet; copy something and it shows up here
history-empty = Nothing yet

# ── Bubbles (the ring's feature list) ─────────────────────
settings-section-bubbles = Order & hotkeys
settings-bubbles-note = Earlier entries unfold first; a disabled bubble leaves the ring, and the rest spread evenly. Each bubble can have its own hotkey — leave it empty for none.
settings-bubbles-name = Name
settings-bubbles-name-hint = Shown in settings; the ring only shows the icon
settings-bubbles-command = Command
settings-bubbles-command-hint = One command line, e.g. notepad.exe or code C:\work
settings-bubbles-icon = Icon
settings-bubbles-icon-default = Default icon
settings-bubbles-icon-pick = Change icon…
settings-bubbles-icon-failed = Cannot import icon
settings-bubbles-remove = Remove this bubble
settings-bubbles-remove-hint = Cannot be undone
settings-section-bubbles-add = Add
settings-bubbles-add-command = Add a command bubble
settings-bubbles-add-command-hint = Adds an empty bubble you can name and point at a command
settings-bubbles-add-program = Add from a program…
settings-bubbles-empty = No bubbles
settings-bubbles-empty-text = empty
settings-bubbles-full = Limit reached ({ $max } max)
settings-bubbles-hotkey-taken = That hotkey already belongs to "{ $name }"
settings-hotkey-none = No hotkey

# ── Custom bubble: launch failure report ──
report-launch-failed = This bubble did not start
report-launch-footer = Fix the command or icon in Settings › Bubbles
