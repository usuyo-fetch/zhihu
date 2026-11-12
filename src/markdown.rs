//! 回答、文章、想法的正文 HTML 换成 Markdown：标题、段落、列表、引用、链接、代码、表格，图片留在原处；
//! 公式留 LaTeX 源码（`$…$`），链接卡片写成普通链接。
//!
//! 来源：由同一个组织里的公众号插件（`usuyo-fetch/wechat-mp`）的 `src/markdown.rs` 改来，转换的骨架一样；
//! 知乎的元素对应（`ztext-math`、`LinkCard`、`video-box`、`data-original`、`link.zhihu.com` 的跳转链接）照知乎页面的写法，
//! 没有对着真实页面量过（见 `src/page.rs` 顶部）。
//!
//! 输出为了能被宿主原样当 Markdown 写进 `content.md`（SPEC §1.2.3）：页面里来的东西只有这个转换自己写出的才是 Markdown 或
//! HTML。文字里的记号加反斜杠、`<` 写成 `&lt;`；链接和图片的目标只认 `http(s)://`（`javascript:`、`data:` 等一律不写成链接，
//! 只留文字）；行内代码的围栏比文字里最长的反引号多一个，代码块同样；公式里会提前结束行内记号的写法（`` ` ``、`](`、`<`、`$`、换行）
//! 改写成等价的 LaTeX。代码块里的 `<` 不改（改了就不是代码），含 `<` 的代码块会让宿主把整篇当纯文字转义，原样在 `raw/`。
//!
//! 图片的目标写页面给的原地址（`https://pic1.zhimg.com/…`），不写下载后的本地路径：相对路径过不了上面那条。
//! 下载下来的文件按同样的次序在输出的 `images` 里，对应关系记在 `meta.json`。

use scraper::{ElementRef, Node};

/// 换出来的正文，加上正文里按出现次序的东西。
#[derive(Debug, Default)]
pub struct Converted {
    pub markdown: String,
    /// 能下载的图片地址，按第一次出现的次序，去重。
    pub images: Vec<String>,
    /// 视频卡片的页面地址（`a.video-box`）：这里不下载。
    pub videos: Vec<String>,
    /// 内嵌框架（`<iframe>`）的地址。
    pub embeds: Vec<String>,
}

/// `is_article_image` 说哪些地址算文章的图片（要下载、记进 `images`）；别的图片地址照样写进 Markdown，不下载。
pub fn convert(root: ElementRef, is_article_image: impl Fn(&str) -> bool) -> Converted {
    let mut walker = Walker {
        is_article_image: &is_article_image,
        out: Converted::default(),
    };
    let mut blocks = Vec::new();
    walker.blocks(root, &mut blocks);
    walker.out.markdown = blocks.join("\n\n");
    walker.out
}

/// 一段纯文字（图片分享、文本分享的正文）：空行分段，段内的换行是硬换行；里面残留的标签（`<a>` 一类）当 HTML 读。
pub fn convert_text(text: &str, is_article_image: impl Fn(&str) -> bool) -> Converted {
    let mut walker = Walker {
        is_article_image: &is_article_image,
        out: Converted::default(),
    };
    let mut blocks = Vec::new();
    for paragraph in text.replace("\r\n", "\n").split("\n\n") {
        let html = scraper::Html::parse_fragment(&paragraph.trim().replace('\n', "<br>"));
        walker.blocks(html.root_element(), &mut blocks);
    }
    walker.out.markdown = blocks.join("\n\n");
    walker.out
}

struct Walker<'a> {
    is_article_image: &'a dyn Fn(&str) -> bool,
    out: Converted,
}

const SKIPPED: &[&str] = &[
    "script", "style", "noscript", "svg", "template", "head", "link", "meta", "title", "canvas",
];

const BLOCKS: &[&str] = &[
    "p",
    "div",
    "section",
    "article",
    "main",
    "header",
    "footer",
    "aside",
    "nav",
    "center",
    "figure",
    "figcaption",
    "dl",
    "dt",
    "dd",
    "address",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "ul",
    "ol",
];

const MORE_BLOCKS: &[&str] = &["blockquote", "pre", "hr", "table", "li", "details"];

const INLINE: &[&str] = &[
    "span", "a", "strong", "b", "em", "i", "u", "s", "del", "strike", "code", "br", "img", "sup",
    "sub", "small", "font", "label", "mark", "abbr", "cite",
];

fn name<'a>(e: ElementRef<'a>) -> &'a str {
    e.value().name()
}

fn is_block(tag: &str) -> bool {
    BLOCKS.contains(&tag) || MORE_BLOCKS.contains(&tag)
}

fn hidden(e: ElementRef) -> bool {
    e.value().attr("style").is_some_and(|s| {
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        s.contains("display:none")
    })
}

fn has_block(e: ElementRef) -> bool {
    e.descendants()
        .filter_map(ElementRef::wrap)
        .any(|d| d != e && (is_block(name(d)) || special(name(d))))
}

fn special(tag: &str) -> bool {
    matches!(tag, "iframe")
}

impl Walker<'_> {
    /// `e` 的孩子换成若干块，追加到 `out`。
    fn blocks(&mut self, e: ElementRef, out: &mut Vec<String>) {
        let mut inline = String::new();
        for child in e.children() {
            match child.value() {
                Node::Text(t) => inline.push_str(&escape(t)),
                Node::Element(_) => {
                    let Some(c) = ElementRef::wrap(child) else {
                        continue;
                    };
                    let tag = name(c);
                    if SKIPPED.contains(&tag) || hidden(c) {
                        continue;
                    }
                    if tag == "a" && card_kind(c).is_some() && has_block(c) {
                        flush(&mut inline, out);
                        let card = self.card(c).unwrap_or_else(|| escape(&card_text(c)));
                        if !card.trim().is_empty() {
                            out.push(card);
                        }
                        continue;
                    }
                    if INLINE.contains(&tag) && !has_block(c) {
                        inline.push_str(&self.inline(c));
                        continue;
                    }
                    flush(&mut inline, out);
                    self.block(c, out);
                }
                _ => {}
            }
        }
        flush(&mut inline, out);
    }

    fn block(&mut self, e: ElementRef, out: &mut Vec<String>) {
        let tag = name(e);
        match tag {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = usize::from(tag.as_bytes()[1] - b'0');
                let text = self.inline_all(e).replace(HARD_BREAK, " ");
                let text = text.trim();
                if !text.is_empty() {
                    out.push(format!("{} {text}", "#".repeat(level)));
                }
            }
            "ul" | "ol" => {
                let list = self.list(e, tag == "ol");
                if !list.is_empty() {
                    out.push(list);
                }
            }
            "blockquote" => {
                let mut inner = Vec::new();
                self.blocks(e, &mut inner);
                if !inner.is_empty() {
                    out.push(quote(&inner.join("\n\n")));
                }
            }
            "pre" => {
                if let Some(code) = fenced(e) {
                    out.push(code);
                }
            }
            "hr" => out.push("---".into()),
            "table" => {
                if let Some(table) = self.table(e) {
                    out.push(table);
                }
            }
            "iframe" => {
                let src = e
                    .value()
                    .attr("data-src")
                    .or_else(|| e.value().attr("src"))
                    .unwrap_or_default();
                if is_web_url(src) {
                    self.out.embeds.push(src.to_owned());
                    out.push(format!("[嵌入内容]({})", link_target(src)));
                }
            }
            // 容器、`li` 单独出现、没认出的标签：只取里面的东西
            _ => self.blocks(e, out),
        }
    }

    /// `e` 的孩子换成一行里的文字（硬换行除外）。
    fn inline_all(&mut self, e: ElementRef) -> String {
        let mut s = String::new();
        for child in e.children() {
            match child.value() {
                Node::Text(t) => s.push_str(&escape(t)),
                Node::Element(_) => {
                    if let Some(c) = ElementRef::wrap(child)
                        && !SKIPPED.contains(&name(c))
                        && !hidden(c)
                    {
                        s.push_str(&self.inline(c));
                    }
                }
                _ => {}
            }
        }
        s
    }

    fn inline(&mut self, e: ElementRef) -> String {
        match name(e) {
            "br" => HARD_BREAK.to_owned(),
            "img" => self.image(e),
            "a" => {
                if card_kind(e).is_some() {
                    return self.card(e).unwrap_or_else(|| escape(&card_text(e)));
                }
                let text = self.inline_all(e).replace(HARD_BREAK, " ");
                match e.value().attr("href").map(str::trim).map(unwrap_redirect) {
                    Some(href) if is_web_url(&href) => {
                        if text.trim().is_empty() {
                            String::new()
                        } else {
                            format!("[{}]({})", text.trim(), link_target(&href))
                        }
                    }
                    _ => text,
                }
            }
            "span" if has_class(e, "ztext-math") => {
                e.value().attr("data-tex").map(math).unwrap_or_default()
            }
            "strong" | "b" => wrap(&self.inline_all(e), "**"),
            "em" | "i" => wrap(&self.inline_all(e), "*"),
            "del" | "s" | "strike" => wrap(&self.inline_all(e), "~~"),
            "code" => {
                let text = plain_text(e);
                let text = text.trim();
                if text.is_empty() {
                    String::new()
                } else {
                    inline_code(text)
                }
            }
            _ => self.inline_all(e),
        }
    }

    fn image(&mut self, e: ElementRef) -> String {
        let v = e.value();
        // 公式的图片版：`alt` 是 LaTeX 源码
        if v.attr("eeimg").is_some()
            && let Some(tex) = v.attr("alt")
        {
            return math(tex);
        }
        // `data-original` 是原图（`_r`），`src` 是缩略图或压缩过的
        let url = [
            v.attr("data-original"),
            v.attr("data-actualsrc"),
            v.attr("data-src"),
            v.attr("src"),
        ]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|u| is_web_url(u) || u.starts_with("//"));
        let Some(url) = url else {
            return String::new();
        };
        let url = decode_amp(&if url.starts_with("//") {
            format!("https:{url}")
        } else {
            url.to_owned()
        });
        if (self.is_article_image)(&url) && !self.out.images.contains(&url) {
            self.out.images.push(url.clone());
        }
        let alt = escape(&v.attr("alt").unwrap_or_default().replace(['[', ']'], ""));
        format!("![{}]({})", alt.trim(), link_target(&url))
    }

    /// 链接卡片和视频卡片：写成一条普通链接；视频只记地址，不下载。
    fn card(&mut self, e: ElementRef) -> Option<String> {
        let href = unwrap_redirect(e.value().attr("href")?.trim());
        if !is_web_url(&href) {
            return None;
        }
        let kind = card_kind(e)?;
        if kind == Card::Video {
            self.out.videos.push(href.clone());
        }
        let text = escape(&card_text(e));
        let text = text.trim();
        let label = match (kind, text.is_empty()) {
            (_, false) => text.to_owned(),
            (Card::Video, true) => "视频".to_owned(),
            (Card::Link, true) => escape(&href),
        };
        Some(format!("[{label}]({})", link_target(&href)))
    }

    fn list(&mut self, e: ElementRef, ordered: bool) -> String {
        let start: usize = e
            .value()
            .attr("start")
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        let mut items = Vec::new();
        for li in e.children().filter_map(ElementRef::wrap) {
            if name(li) != "li" || hidden(li) {
                continue;
            }
            let mut inner = Vec::new();
            self.blocks(li, &mut inner);
            if inner.is_empty() {
                continue;
            }
            let marker = if ordered {
                format!("{}. ", start + items.len())
            } else {
                "- ".to_owned()
            };
            let pad = " ".repeat(marker.len());
            let body = inner.join("\n");
            let mut lines = body.lines();
            let mut item = format!("{marker}{}", lines.next().unwrap_or_default());
            for line in lines {
                item.push('\n');
                if !line.is_empty() {
                    item.push_str(&pad);
                    item.push_str(line);
                }
            }
            items.push(item);
        }
        items.join("\n")
    }

    fn table(&mut self, e: ElementRef) -> Option<String> {
        let mut rows: Vec<Vec<String>> = Vec::new();
        for tr in e
            .descendants()
            .filter_map(ElementRef::wrap)
            .filter(|d| name(*d) == "tr")
        {
            let cells: Vec<String> = tr
                .children()
                .filter_map(ElementRef::wrap)
                .filter(|c| matches!(name(*c), "td" | "th"))
                .map(|c| {
                    self.inline_all(c)
                        .replace(HARD_BREAK, " ")
                        .replace('|', "\\|")
                        .trim()
                        .to_owned()
                })
                .collect();
            if !cells.is_empty() {
                rows.push(cells);
            }
        }
        let width = rows.iter().map(Vec::len).max()?;
        let line = |cells: &[String]| {
            let mut cells = cells.to_vec();
            cells.resize(width, String::new());
            format!("| {} |", cells.join(" | "))
        };
        let mut out = vec![line(&rows[0]), format!("|{}", " --- |".repeat(width))];
        out.extend(rows[1..].iter().map(|r| line(r)));
        Some(out.join("\n"))
    }
}

/// 行内的硬换行占位：`flush` 换成 Markdown 的硬换行。
const HARD_BREAK: &str = "\u{1}";

/// 一段行内文字收成一个块。空的不收。
fn flush(inline: &mut String, out: &mut Vec<String>) {
    let text = std::mem::take(inline);
    let mut lines: Vec<String> = text
        .split(HARD_BREAK)
        .map(|l| merge_bold(l.trim()))
        .map(|l| escape_line_start(&l))
        .collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    while lines.first().is_some_and(String::is_empty) {
        lines.remove(0);
    }
    if !lines.is_empty() {
        out.push(lines.join("  \n"));
    }
}

/// 相邻的两段加粗（`**a****b**`）合成一段。
fn merge_bold(line: &str) -> String {
    line.replace("****", "")
}

fn wrap(inner: &str, mark: &str) -> String {
    let trimmed = inner.trim();
    if trimmed.is_empty() || trimmed.contains(HARD_BREAK) {
        return inner.to_owned();
    }
    let lead = &inner[..inner.len() - inner.trim_start().len()];
    let trail = &inner[inner.trim_end().len()..];
    format!("{lead}{mark}{trimmed}{mark}{trail}")
}

fn quote(text: &str) -> String {
    text.lines()
        .map(|l| {
            if l.is_empty() {
                ">".to_owned()
            } else {
                format!("> {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 行首的 `#`、`>`、`-`、`+`、`1.` 会被当成块标记，前面加反斜杠。
fn escape_line_start(line: &str) -> String {
    let t = line.trim_start();
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if digits > 0
        && t[digits..]
            .strip_prefix(['.', ')'])
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
    {
        return format!("{}\\{}", &t[..digits], &t[digits..]);
    }
    let hashes = t.chars().take_while(|&c| c == '#').count();
    let heading = (1..=6).contains(&hashes) && t[hashes..].starts_with([' ', '\u{1}']) || t == "#";
    let bullet =
        matches!(t.chars().next(), Some('-' | '+')) && (t.len() == 1 || t[1..].starts_with(' '));
    let rule = !t.is_empty() && (t.chars().all(|c| c == '=') || t.chars().all(|c| c == '-'));
    if heading || bullet || rule || t.starts_with('>') {
        return format!("\\{t}");
    }
    t.to_owned()
}

/// 文字里的空白收成一个空格，Markdown 的记号加反斜杠，`<` 写成实体。
fn escape(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut last_space = false;
    for (i, &c) in chars.iter().enumerate() {
        if c.is_whitespace() {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
            continue;
        }
        last_space = false;
        match c {
            '\\' | '`' | '*' | '[' | ']' => {
                out.push('\\');
                out.push(c);
            }
            // 两边都是字母数字的 `_`（`snake_case`）不当强调
            '_' => {
                let inside = i > 0
                    && chars[i - 1].is_alphanumeric()
                    && chars.get(i + 1).is_some_and(|n| n.is_alphanumeric());
                if !inside {
                    out.push('\\');
                }
                out.push('_');
            }
            '~' if chars.get(i + 1) == Some(&'~') || (i > 0 && chars[i - 1] == '~') => {
                out.push_str("\\~");
            }
            '<' => out.push_str("&lt;"),
            _ => out.push(c),
        }
    }
    out
}

/// 代码块里的文字：`<br>` 是换行，不间断空格是空格，别的空白原样。
fn plain_text(e: ElementRef) -> String {
    let mut s = String::new();
    for node in e.descendants() {
        match node.value() {
            Node::Text(t) => s.push_str(&t.replace('\u{a0}', " ")),
            Node::Element(el) if el.name() == "br" => s.push('\n'),
            _ => {}
        }
    }
    s
}

fn class_lang<'a>(e: ElementRef<'a>) -> Option<&'a str> {
    e.value()
        .classes()
        .find_map(|c| c.strip_prefix("language-"))
}

/// `<pre>`：知乎的代码块是一个 `<code>`，语言写在 `class="language-…"` 上；也认一行一个 `<code>` 的写法。
fn fenced(pre: ElementRef) -> Option<String> {
    let codes: Vec<ElementRef> = pre
        .children()
        .filter_map(ElementRef::wrap)
        .filter(|c| name(*c) == "code")
        .collect();
    let text = if codes.is_empty() {
        plain_text(pre)
    } else {
        codes
            .iter()
            .map(|c| plain_text(*c))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let text = text
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim_matches('\n');
    if text.trim().is_empty() {
        return None;
    }
    let lang = pre
        .value()
        .attr("data-lang")
        .or_else(|| pre.value().attr("lang"))
        .or_else(|| class_lang(pre))
        .or_else(|| {
            codes
                .first()
                .and_then(|c| c.value().attr("data-lang").or_else(|| class_lang(*c)))
        })
        .map(|l| l.split_whitespace().next().unwrap_or_default())
        .filter(|l| {
            l.chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-#._".contains(c))
        })
        .unwrap_or_default();
    let mut fence = "```".to_owned();
    while text.contains(&fence) {
        fence.push('`');
    }
    Some(format!("{fence}{lang}\n{text}\n{fence}"))
}

/// 行内代码：围栏比文字里最长的一串反引号多一个；文字以反引号开头或结尾时两边各垫一个空格
/// （CommonMark 的规则）。这样正文里的反引号关不掉围栏，后面的字不会被当成 Markdown。
fn inline_code(text: &str) -> String {
    let longest = text
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    let fence = "`".repeat(longest + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Card {
    Link,
    Video,
}

fn has_class(e: ElementRef, class: &str) -> bool {
    e.value().classes().any(|c| c == class)
}

fn card_kind(e: ElementRef) -> Option<Card> {
    if has_class(e, "video-box") {
        Some(Card::Video)
    } else if has_class(e, "LinkCard") || e.value().attr("data-draft-type") == Some("link-card") {
        Some(Card::Link)
    } else {
        None
    }
}

/// 卡片上的字：只取文字，不取里面的图片，空白收成一个空格。
fn card_text(e: ElementRef) -> String {
    let text: String = e
        .descendants()
        .filter_map(|n| match n.value() {
            Node::Text(t) => Some(t.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `https://link.zhihu.com/?target=<编码过的地址>` 是知乎的外链跳转，取出里面的地址；别的链接原样。
fn unwrap_redirect(href: &str) -> String {
    let Ok(url) = url::Url::parse(&if href.starts_with("//") {
        format!("https:{href}")
    } else {
        href.to_owned()
    }) else {
        return href.to_owned();
    };
    if url.host_str() == Some("link.zhihu.com")
        && let Some((_, target)) = url.query_pairs().find(|(k, _)| k == "target")
    {
        return target.into_owned();
    }
    href.to_owned()
}

/// 公式：LaTeX 源码放在 `$…$` 里。会提前结束行内记号或变成 HTML 的几处改写成等价的 LaTeX：
/// 换行换成空格，`$`、`` ` `` 加反斜杠，`<` 写成 `\lt{}`，`](` 在中间垫一个空组 `]{}(`。
fn math(tex: &str) -> String {
    let tex = tex.split_whitespace().collect::<Vec<_>>().join(" ");
    if tex.is_empty() {
        return String::new();
    }
    let tex = tex
        .replace('$', "\\$")
        .replace('`', "\\`")
        .replace('<', "\\lt{}")
        .replace("](", "]{}(");
    format!("${tex}$")
}

fn is_web_url(s: &str) -> bool {
    let l = s.trim().to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

/// 页面把地址里的 `&` 写成 `&amp;`（有时写两层）。
pub fn decode_amp(s: &str) -> String {
    let mut s = s.to_owned();
    while s.contains("&amp;") {
        s = s.replace("&amp;", "&");
    }
    s
}

/// 链接目标里空格、括号、尖括号会截断 Markdown 的解析，百分号编码。
fn link_target(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for c in url.trim().chars() {
        match c {
            ' ' => out.push_str("%20"),
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '\\' => out.push_str("%5C"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use scraper::Html;

    use super::*;

    fn md(html: &str) -> Converted {
        let doc = Html::parse_fragment(html);
        convert(doc.root_element(), |u| u.contains("zhimg.com"))
    }

    #[test]
    fn headings_paragraphs_emphasis() {
        let c =
            md("<h2>标题</h2><p data-pid=\"a\">前 <b>粗</b><b>体</b> 后<br>下一行</p><p> </p><hr>");
        assert_eq!(c.markdown, "## 标题\n\n前 **粗体** 后  \n下一行\n\n---");
    }

    #[test]
    fn lists_quotes_tables() {
        let c = md(
            "<ul><li><p>一</p><ul><li>一之一</li></ul></li><li>二</li></ul>\
             <ol start=\"3\"><li>三</li></ol>\
             <blockquote><p>引</p></blockquote>\
             <table><tr><th>甲</th><th>乙</th></tr><tr><td>1|2</td><td>3</td></tr></table>",
        );
        assert_eq!(
            c.markdown,
            "- 一\n  - 一之一\n- 二\n\n3. 三\n\n> 引\n\n| 甲 | 乙 |\n| --- | --- |\n| 1\\|2 | 3 |"
        );
    }

    #[test]
    fn code_blocks_keep_the_language_and_cannot_be_closed_by_their_text() {
        let c = md(
            "<div class=\"highlight\"><pre><code class=\"language-python\">print(1)\nif a &lt; 2:\n    pass</code></pre></div>\
             <pre lang=\"rust\"><code>let s = \"```\";</code></pre>",
        );
        assert_eq!(
            c.markdown,
            "```python\nprint(1)\nif a < 2:\n    pass\n```\n\n````rust\nlet s = \"```\";\n````"
        );
    }

    #[test]
    fn formulas_stay_as_latex_source() {
        let c = md(
            "<p>设 <span class=\"ztext-math\" data-tex=\"x^2 + y_1 = \\frac{a}{b}\"><span class=\"MathJax_Preview\">x</span></span> 成立，\
             又 <img src=\"//www.zhihu.com/equation?tex=a\" alt=\"\\alpha&lt;\\beta\" eeimg=\"1\"></p>",
        );
        assert_eq!(
            c.markdown,
            "设 $x^2 + y_1 = \\frac{a}{b}$ 成立，又 $\\alpha\\lt{}\\beta$"
        );
        assert!(c.images.is_empty());
    }

    #[test]
    fn a_formula_cannot_end_its_own_markers_or_open_a_link_or_html() {
        let c = md(
            "<p><span class=\"ztext-math\" data-tex=\"a $ b ` c\n[x](javascript:alert(1)) <script>x</script>\"></span></p>",
        );
        assert!(!c.markdown.contains('<'), "{}", c.markdown);
        assert!(!c.markdown.contains("]("), "{}", c.markdown);
        assert_eq!(
            c.markdown,
            "$a \\$ b \\` c [x]{}(javascript:alert(1)) \\lt{}script>x\\lt{}/script>$"
        );
    }

    #[test]
    fn images_use_the_original_size_in_place_and_in_order() {
        let c = md(
            "<p>a</p><figure><noscript><img src=\"https://pic1.zhimg.com/v2-aaaa_720w.jpg\"></noscript>\
             <img src=\"data:image/svg+xml;utf8,&lt;svg/&gt;\" data-actualsrc=\"https://pic1.zhimg.com/v2-aaaa_720w.jpg\" \
              data-original=\"https://pic1.zhimg.com/v2-aaaa_r.jpg\" alt=\"图[1]\">\
             <figcaption>图一</figcaption></figure>\
             <p><img src=\"https://static.zhihu.com/e.png\"><img data-original=\"https://pic1.zhimg.com/v2-aaaa_r.jpg\"></p>\
             <img src=\"https://picx.zhimg.com/50/v2-bbbb_hd.jpg?source=1&amp;x=2\">",
        );
        assert_eq!(
            c.images,
            [
                "https://pic1.zhimg.com/v2-aaaa_r.jpg",
                "https://picx.zhimg.com/50/v2-bbbb_hd.jpg?source=1&x=2",
            ]
        );
        assert!(
            c.markdown
                .starts_with("a\n\n![图1](https://pic1.zhimg.com/v2-aaaa_r.jpg)\n\n图一\n\n"),
            "{}",
            c.markdown
        );
        assert!(c.markdown.contains("![](https://static.zhihu.com/e.png)"));
    }

    #[test]
    fn link_cards_become_plain_links_and_video_cards_are_only_recorded() {
        let c = md(
            "<p><a class=\"LinkCard\" href=\"https://link.zhihu.com/?target=https%3A//example.com/a%3Fb%3D1\" \
              data-draft-type=\"link-card\"><span>示例站点</span><span>example.com</span></a></p>\
             <p>见 <a href=\"https://link.zhihu.com/?target=https%3A//example.org/x\">这里</a></p>\
             <a class=\"video-box\" href=\"https://www.zhihu.com/video/123\"><div><img src=\"https://pic1.zhimg.com/v2-v_r.jpg\"></div><span>一段视频</span></a>",
        );
        assert_eq!(
            c.markdown,
            "[示例站点 example.com](https://example.com/a?b=1)\n\n见 [这里](https://example.org/x)\n\n[一段视频](https://www.zhihu.com/video/123)"
        );
        assert_eq!(c.videos, ["https://www.zhihu.com/video/123"]);
        assert!(c.images.is_empty());
    }

    #[test]
    fn text_is_escaped_and_has_no_angle_brackets() {
        let c = md("<p>1 &lt; 2 * 3 _x_ [y] snake_case # a</p><p># 不是标题</p><p>2. 不是列表</p>");
        assert_eq!(
            c.markdown,
            "1 &lt; 2 \\* 3 \\_x\\_ \\[y\\] snake_case # a\n\n\\# 不是标题\n\n2\\. 不是列表"
        );
    }

    #[test]
    fn inline_code_cannot_be_closed_by_its_own_text() {
        assert_eq!(inline_code("x_y"), "`x_y`");
        assert_eq!(inline_code("a`b"), "``a`b``");
        assert_eq!(
            inline_code("a``b [点这里](https://example.com)"),
            "```a``b [点这里](https://example.com)```"
        );
        assert_eq!(inline_code("`x`"), "`` `x` ``");
        let c = md("<p><code>a``b [x](https://e.test)</code> 后面 <b>粗</b></p>");
        assert_eq!(c.markdown, "```a``b [x](https://e.test)``` 后面 **粗**");
    }

    #[test]
    fn hostile_text_never_becomes_a_link_image_or_html() {
        let c = md(
            "<p>](https://evil.test) [a](https://evil.test) ![x](https://evil.test/i.png) <script>alert(1)</script></p>\
             <p><a href=\"javascript:alert(1)\">点我</a> <a href=\"data:text/html,x\">数据</a> <a href=\"mailto:a@b.test\">邮件</a> \
              <a href=\"https://link.zhihu.com/?target=javascript%3Aalert(1)\">跳</a> <a href=\"//ok.test/a\">协议相对</a></p>\
             <p><img src=\"https://pic1.zhimg.com/a_r.jpg\" alt=\"&lt;script&gt;x&lt;/script&gt;](https://evil.test)`\"></p>\
             <a class=\"LinkCard\" href=\"javascript:alert(1)\">卡</a>\
             <h3>&lt;img src=x onerror=1&gt;</h3>\
             <blockquote>&lt;b&gt;引&lt;/b&gt;</blockquote>",
        );
        assert_eq!(
            c.markdown,
            "\\](https://evil.test) \\[a\\](https://evil.test) !\\[x\\](https://evil.test/i.png)\n\n\
             点我 数据 邮件 跳 协议相对\n\n\
             ![&lt;script>x&lt;/script>(https://evil.test)\\`](https://pic1.zhimg.com/a_r.jpg)\n\n\
             卡\n\n### &lt;img src=x onerror=1>\n\n> &lt;b>引&lt;/b>"
        );
        assert!(!c.markdown.contains('<'));
    }

    #[test]
    fn nothing_from_a_page_closes_a_code_block() {
        let c = md("<pre><code>a\n```\n[x](javascript:1)\n````\nb</code></pre>");
        assert_eq!(
            c.markdown,
            "`````\na\n```\n[x](javascript:1)\n````\nb\n`````"
        );
    }

    #[test]
    fn plain_text_posts_keep_lines_and_paragraphs() {
        let c = convert_text(
            "第一行\n第二行\n\n第三段 <a href=\"https://e.test/\">链</a>\n",
            |_| false,
        );
        assert_eq!(
            c.markdown,
            "第一行  \n第二行\n\n第三段 [链](https://e.test/)"
        );
    }
}
