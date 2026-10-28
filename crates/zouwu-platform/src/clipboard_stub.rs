//! 非 Windows 占位：剪贴板未接入。

pub fn set_text(_text: &str) -> bool {
    false
}

pub fn get_text() -> Option<String> {
    None
}

pub fn sequence() -> Option<u32> {
    None
}
