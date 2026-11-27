//! 启动器：搜索本机已安装的程序（纯逻辑，不含 IO）。
//!
//! 程序清单从哪来由平台层负责（`zouwu_platform::programs`），这里只管
//! **怎么排**：同一批程序，搜 `we` 时用户想要的顺序几乎是确定的。
//!
//! 排序按"人工判断的强度"分档，而不是算一个综合分——档位之间差得足够大，
//! 综合分那种加权组合反而会因为词多词少而翻车：
//!
//! | 情况 | 分 |
//! |---|---|
//! | 名字完全等于关键词 | 100 |
//! | 名字以关键词开头 | 80 |
//! | 名字里某个词以关键词开头 | 60 |
//! | 名字包含关键词 | 40 |
//! | 路径包含关键词 | 20 |
//! | 名字按顺序散落包含这些字母 | 10 |
//!
//! 多个关键词取"与"（每个都要命中），分数相加。同分时短名字优先：
//! 搜 `code` 时 `Code` 该排在 `Code::Blocks` 前面。

/// 本机已安装的一个程序。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    /// 显示名（开始菜单里那个名字），搜索主要看它。
    pub name: String,
    /// 目标的完整路径（`.lnk` 或 `.exe`），路径也参与搜索。
    pub path: String,
}

impl Program {
    pub fn new(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
        }
    }
}

/// 命中关键词的强度，`None` = 没命中。
fn term_score(term: &str, name: &str, path: &str) -> Option<u32> {
    if name == term {
        return Some(100);
    }
    if name.starts_with(term) {
        return Some(80);
    }
    // 词首命中：`visual studio code` 里的 `studio`。
    if name
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| !word.is_empty() && word.starts_with(term))
    {
        return Some(60);
    }
    if name.contains(term) {
        return Some(40);
    }
    if path.contains(term) {
        return Some(20);
    }
    if subsequence(name, term) {
        return Some(10);
    }
    None
}

/// `needle` 的字符是否按顺序出现在 `hay` 里（不要求连续）。
fn subsequence(hay: &str, needle: &str) -> bool {
    let mut rest = hay.chars();
    needle.chars().all(|c| rest.any(|h| h == c))
}

/// 命中的程序下标，按"最可能是用户想要的那个"排好。
///
/// 空查询返回全部（原顺序）——打开启动器还没打字时应该看到整个清单。
pub fn search(programs: &[Program], query: &str) -> Vec<usize> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if terms.is_empty() {
        return (0..programs.len()).collect();
    }
    let mut hits: Vec<(usize, u32)> = Vec::new();
    for (index, program) in programs.iter().enumerate() {
        let name = program.name.to_lowercase();
        let path = program.path.to_lowercase();
        let mut total = 0;
        let mut all = true;
        for term in &terms {
            match term_score(term, &name, &path) {
                Some(score) => total += score,
                None => {
                    all = false;
                    break;
                }
            }
        }
        if all {
            hits.push((index, total));
        }
    }
    hits.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| {
                programs[a.0]
                    .name
                    .chars()
                    .count()
                    .cmp(&programs[b.0].name.chars().count())
            })
            .then_with(|| programs[a.0].name.cmp(&programs[b.0].name))
    });
    hits.into_iter().map(|(index, _)| index).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Program> {
        vec![
            Program::new("Visual Studio Code", r"C:\Menu\Visual Studio Code.lnk"),
            Program::new("Code::Blocks", r"C:\Menu\CodeBlocks.lnk"),
            Program::new("Calculator", r"C:\Menu\Calculator.lnk"),
            Program::new("Notepad", r"C:\Menu\Notepad.lnk"),
            Program::new("WeChat", r"C:\Menu\Tencent\WeChat.lnk"),
        ]
    }

    fn names(hits: Vec<usize>, programs: &[Program]) -> Vec<String> {
        hits.into_iter().map(|i| programs[i].name.clone()).collect()
    }

    #[test]
    fn empty_query_lists_everything_in_order() {
        let programs = sample();
        assert_eq!(search(&programs, ""), vec![0, 1, 2, 3, 4]);
        assert_eq!(search(&programs, "   "), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn exact_name_beats_prefix_and_prefix_beats_substring() {
        let programs = sample();
        let got = names(search(&programs, "code"), &programs);
        // 没有哪条名字正好叫 code：前缀档（Code::Blocks）走在包含档（Visual Studio Code）前面。
        assert_eq!(got, vec!["Code::Blocks", "Visual Studio Code"]);
    }

    #[test]
    fn shorter_name_wins_on_a_tie() {
        let programs = vec![
            Program::new("Notepad++", r"C:\Menu\Notepad++.lnk"),
            Program::new("Notepad", r"C:\Menu\Notepad.lnk"),
        ];
        let got = names(search(&programs, "notepad"), &programs);
        // 前缀档同分：短的那个先。
        assert_eq!(got, vec!["Notepad", "Notepad++"]);
    }

    #[test]
    fn every_term_must_hit() {
        let programs = sample();
        assert_eq!(
            names(search(&programs, "code visual"), &programs),
            vec!["Visual Studio Code"]
        );
        assert!(search(&programs, "code zzzz").is_empty());
    }

    #[test]
    fn path_matches_are_the_last_stop_before_fuzzy() {
        let programs = vec![
            Program::new("QQ", r"C:\Menu\Tencent\QQ.lnk"),
            Program::new("WeChat", r"C:\Menu\Tencent\WeChat.lnk"),
        ];
        // `tencent` 只在两条的路径里：都命中，排序按名字长度稳定下来。
        let got = names(search(&programs, "tencent"), &programs);
        assert_eq!(got, vec!["QQ", "WeChat"]);
    }

    #[test]
    fn scattered_letters_still_find_the_program() {
        let programs = sample();
        let got = names(search(&programs, "wct"), &programs);
        assert_eq!(got, vec!["WeChat"]);
    }

    #[test]
    fn case_is_ignored() {
        let programs = sample();
        assert_eq!(search(&programs, "CALC"), search(&programs, "calc"));
        assert!(!search(&programs, "CALC").is_empty());
    }

    #[test]
    fn subsequence_requires_order() {
        assert!(subsequence("wechat", "wct"));
        assert!(!subsequence("wechat", "ctw"));
    }

    #[test]
    fn term_score_tiers_are_ordered() {
        let exact = term_score("code", "code", "").unwrap();
        let prefix = term_score("code", "codeblocks", "").unwrap();
        let word = term_score("block", "code::blocks", "").unwrap();
        let substring = term_score("ode", "codeblocks", "").unwrap();
        let path = term_score("menu", "codeblocks", r"c:\menu\cb.lnk").unwrap();
        let fuzzy = term_score("cbl", "codeblocks", "").unwrap();
        assert!(exact > prefix);
        assert!(prefix > word);
        assert!(word > substring);
        assert!(substring > path);
        assert!(path > fuzzy);
    }
}
