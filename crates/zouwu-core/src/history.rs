//! 剪贴板历史。
//!
//! 只做**文本**：图片剪贴板要另存文件、要缩略图、要处理 GDI+ 解码，
//! 收益和成本不成比例，先留给后续（配置里已经留了开关位）。
//!
//! 三条规则决定了这个文件的形态：
//!
//! 1. **同样的内容不重复占位**。复制同一段文字三次只留一条，并且提到最前——
//!    否则历史里会堆满同一份内容，翻起来全是重复项。
//! 2. **条目是有上限的**。剪贴板里可能是一整个日志文件，没有上限时内存和
//!    磁盘都会被慢慢吃掉。超限时丢最旧的，而不是拒绝新的（用户不会关心
//!    第 101 条，但会在意"我刚复制的没进去"）。
//! 3. **单条也只留前面一段**。一行 200KB 的 base64 存进去没有使用价值，
//!    翻历史时还要为它渲染一屏。截断是明智的损失。

use serde::{Deserialize, Serialize};

/// 配置写坏时的兜底值。
pub const DEFAULT_LIMIT: usize = 100;

/// 能接受的极限：再大就不要了（挡住"复制了一整个日志文件"）。
pub const LIMIT_MAX: usize = 500;

/// 单条文本最多保留的字符数（按 `char` 计，不是字节）。
pub const ENTRY_MAX_CHARS: usize = 2_000;

/// 一次 `push` 的结果。
///
/// 必须区分"新增"和"只是提到了最前"：调用方要据此决定是否落盘、是否打日志。
/// 早期版本用 `bool` 表示"是否新增"，于是重复内容被提位之后**没有落盘** ——
/// 内存里顺序对了，重启又变回去。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Push {
    /// 新内容，占了一行。
    Added,
    /// 老内容被提到了最前（不新增行）。
    Moved,
    /// 空白/无效内容，什么也没发生。
    Ignored,
}

/// 历史文件的 schema 版本。
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub text: String,
    /// Unix 秒。只用于排序与"多久以前"的显示，不要求高精度。
    pub copied_at: u64,
}

impl Entry {
    pub fn new(text: impl Into<String>, copied_at: u64) -> Self {
        Self {
            text: text.into(),
            copied_at,
        }
    }

    /// 列表里显示的一行：第一条非空行，压缩连续空白。
    ///
    /// 多行内容（代码片段、整段文件）在列表里只露一行，这是刻意的——
    /// 一行足够让用户认出"是不是我要的那条"，而完整内容在选中后的预览里。
    pub fn preview(&self) -> String {
        let squash = |line: &str| line.split_whitespace().collect::<Vec<_>>().join(" ");
        self.text
            .lines()
            .map(squash)
            .find(|line| !line.is_empty())
            .unwrap_or_default()
    }

    /// 是否是多行内容（列表里给个记号）。
    pub fn is_multiline(&self) -> bool {
        self.text.trim_end().contains('\n')
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    pub entries: Vec<Entry>,
    /// 用户配置的容量。放在这里而不是每次从 `Config` 取，是为了让
    /// "截断到上限"由数据自己负责——否则每个调用点都要记得传。
    pub limit: usize,
}

impl History {
    pub fn new(limit: usize) -> Self {
        Self {
            entries: Vec::new(),
            limit: clamp_limit(limit),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&Entry> {
        self.entries.get(index)
    }

    pub fn set_limit(&mut self, limit: usize) {
        self.limit = clamp_limit(limit);
        self.entries.truncate(self.limit);
    }

    /// 记一条。
    ///
    /// 空白内容直接忽略：某些程序在启动/切换焦点时会往剪贴板塞空字符串，
    /// 把它当历史会让列表迅速变脏。
    pub fn push(&mut self, text: &str, now: u64) -> Push {
        let text = normalize(text);
        if text.is_empty() {
            return Push::Ignored;
        }
        // 已经存在：提到最前并刷新时间，不新增一条。
        // 已经在最前时不重复搬运（避免无谓的落盘）。
        if let Some(index) = self.entries.iter().position(|e| e.text == text) {
            if index == 0 {
                self.entries[0].copied_at = now;
                return Push::Ignored;
            }
            let mut entry = self.entries.remove(index);
            entry.copied_at = now;
            self.entries.insert(0, entry);
            return Push::Moved;
        }
        self.entries.insert(0, Entry::new(text, now));
        self.entries.truncate(self.limit);
        Push::Added
    }

    /// 删掉某一条（用户在历史窗里按 Delete）。
    pub fn remove(&mut self, index: usize) -> Option<Entry> {
        if index < self.entries.len() {
            Some(self.entries.remove(index))
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// 序列化成 `history.toml`。
    ///
    /// 不序列化 `limit`：那是配置（`config.toml` 的 `clipboard.limit`），
    /// 存在历史文件里会出现两份真相。
    pub fn to_toml(&self) -> String {
        let file = HistoryFile {
            schema_version: SCHEMA_VERSION,
            entries: self.entries.clone(),
        };
        toml::to_string_pretty(&file).unwrap_or_default()
    }

    /// 解析历史文件。坏文件**不**返回错误——历史丢了是可以接受的小事，
    /// 让启动因此失败则是大事。
    pub fn from_toml(text: &str) -> Self {
        let file: HistoryFile = match toml::from_str(text) {
            Ok(file) => file,
            Err(err) => {
                log::warn!("history file is unreadable: {err}");
                return History::new(DEFAULT_LIMIT);
            }
        };
        let mut history = History::new(DEFAULT_LIMIT);
        // 文件里的顺序就是新旧顺序，但上限未知——先全收，交给调用方 set_limit 裁剪。
        history.entries = file.entries;
        history
    }
}

#[derive(Serialize, Deserialize)]
struct HistoryFile {
    schema_version: u32,
    #[serde(default)]
    entries: Vec<Entry>,
}

fn clamp_limit(limit: usize) -> usize {
    // 0 会被当作"不要历史"，但那样每复制一次就清空一次，容易让用户
    // 误以为功能坏了；最小 1 条，关闭功能请走 `clipboard.enabled`。
    limit.clamp(1, LIMIT_MAX)
}

/// 归一化要入库的文本：统一换行、去首尾空白、限长。
fn normalize(text: &str) -> String {
    let trimmed = text.trim();
    let unified: String = trimmed.replace("\r\n", "\n").replace('\r', "\n");
    if unified.chars().count() <= ENTRY_MAX_CHARS {
        return unified;
    }
    // 按字符截断，别按字节：按字节会把一个汉字劈成两半，
    // 存进去的是非法 UTF-8 前缀，接出来就是乱码。
    let mut out: String = unified.chars().take(ENTRY_MAX_CHARS).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_entry_comes_first() {
        let mut history = History::new(10);
        history.push("one", 1);
        history.push("two", 2);
        assert_eq!(history.get(0).unwrap().text, "two");
        assert_eq!(history.get(1).unwrap().text, "one");
    }

    #[test]
    fn repeating_a_value_moves_it_up_instead_of_duplicating() {
        let mut history = History::new(10);
        history.push("a", 1);
        history.push("b", 2);
        let outcome = history.push("a", 3);
        assert_eq!(outcome, Push::Moved, "repeat must not create a row");
        assert_eq!(history.len(), 2);
        assert_eq!(history.get(0).unwrap().text, "a");
        assert_eq!(history.get(0).unwrap().copied_at, 3);
    }

    #[test]
    fn blank_entries_are_ignored() {
        let mut history = History::new(10);
        assert_eq!(history.push("", 1), Push::Ignored);
        assert_eq!(history.push("  \r\n\t ", 2), Push::Ignored);
        assert!(history.is_empty());
    }

    #[test]
    fn oldest_entry_is_dropped_at_the_limit() {
        let mut history = History::new(2);
        assert_eq!(history.push("a", 1), Push::Added);
        history.push("b", 2);
        history.push("c", 3);
        assert_eq!(history.len(), 2);
        assert_eq!(
            history
                .entries
                .iter()
                .map(|e| e.text.as_str())
                .collect::<Vec<_>>(),
            vec!["c", "b"]
        );
    }

    #[test]
    fn long_text_is_cut_on_a_character_boundary() {
        let mut history = History::new(4);
        // 全角汉字：按字节截断会切出半个字符。
        history.push(&"永".repeat(ENTRY_MAX_CHARS + 50), 1);
        let text = &history.get(0).unwrap().text;
        assert_eq!(text.chars().count(), ENTRY_MAX_CHARS + 1, "cut + ellipsis");
        assert!(text.ends_with('…'));
        assert!(text.is_char_boundary(text.len()));
    }

    #[test]
    fn windows_newlines_are_normalized() {
        let mut history = History::new(4);
        history.push("a\r\nb\rc", 1);
        assert_eq!(history.get(0).unwrap().text, "a\nb\nc");
        assert!(history.get(0).unwrap().is_multiline());
    }

    #[test]
    fn preview_shows_the_first_non_empty_line() {
        let entry = Entry::new("\n\n  你好   世界  \n第二行", 1);
        assert_eq!(entry.preview(), "你好 世界");
        assert!(entry.is_multiline());
    }

    #[test]
    fn pushing_the_top_entry_again_reports_ignored() {
        let mut history = History::new(4);
        history.push("a", 1);
        assert_eq!(history.push("a", 2), Push::Ignored);
        assert_eq!(
            history.get(0).unwrap().copied_at,
            2,
            "timestamp still refreshes"
        );
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn lowering_the_limit_trims_existing_entries() {
        let mut history = History::new(10);
        for i in 0..6 {
            history.push(&format!("entry {i}"), i);
        }
        history.set_limit(3);
        assert_eq!(history.len(), 3);
        assert_eq!(history.get(0).unwrap().text, "entry 5");
    }

    #[test]
    fn limit_is_clamped_to_a_sane_range() {
        assert_eq!(History::new(0).limit, 1, "zero would silently wipe history");
        assert_eq!(History::new(usize::MAX).limit, LIMIT_MAX);
    }

    #[test]
    fn history_file_roundtrips_without_the_limit() {
        let mut history = History::new(42);
        history.push("hello", 7);
        let text = history.to_toml();
        assert!(
            !text.contains("limit"),
            "limit belongs to config.toml, not the history file"
        );

        let mut loaded = History::from_toml(&text);
        loaded.set_limit(42);
        assert_eq!(loaded.entries, history.entries);
    }

    #[test]
    fn broken_history_file_degrades_to_empty_history() {
        let loaded = History::from_toml("this is not toml {{{");
        assert!(loaded.is_empty());
    }
}
