/// 分类元数据：单一数据源，驱动 UI 按钮渲染与键盘循环。
/// 未来改动态分类只需把 CATEGORIES 换成运行时 Vec。
pub struct CategoryDef {
    pub id: &'static str,
    pub label: &'static str,
}

pub const CATEGORIES: &[CategoryDef] = &[
    CategoryDef { id: "all", label: "全部" },
    CategoryDef { id: "marked", label: "标记" },
    CategoryDef { id: "text", label: "文本" },
    CategoryDef { id: "image", label: "图片" },
    CategoryDef { id: "files", label: "文件" },
];

pub const DEFAULT_CATEGORY: &str = "all";

/// 循环到下一个分类：direction=1 向右，-1 向左，越界回绕。
/// 找不到 current 时返回首项，保证健壮。
pub fn cycle(current: &str, direction: i32) -> &'static str {
    if CATEGORIES.is_empty() {
        return DEFAULT_CATEGORY;
    }
    let idx = CATEGORIES.iter().position(|c| c.id == current).unwrap_or(0);
    let len = CATEGORIES.len() as i32;
    let next = (idx as i32 + direction).rem_euclid(len) as usize;
    CATEGORIES[next].id
}
