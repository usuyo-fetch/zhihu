//! 读回答、文章、想法的页面：取出页面内嵌的数据（`<script id="js-initialData" type="text/json">`，不运行脚本），
//! 认出这一页是什么、标题、作者、时间、计数和正文。
//!
//! 内嵌数据的外层结构（`initialState.entities.{answers,articles,pins,questions,…}` 按 id 存）对着 2026-11-12 取到的公开收藏夹页量过：
//! `entities.favlists.<id>` 是收藏夹自己的信息，同一个 `entities` 下还有 `answers`、`articles`、`pins`、`questions`、`users` 等
//! 空对象。回答、文章、想法的条目里有哪些字段，没有对着真实页面量过（这台机器取到的回答、文章页是挑战页，见 `src/api.rs` 顶部），
//! 下面的字段名是照知乎页面和上游项目的常见写法写的，测试夹具是手写的：
//! - 回答：`answers.<id>`，`question.{id,title}`、`author`、`content`（HTML）、`createdTime`、`updatedTime`、各种 `…Count`；
//! - 文章：`articles.<id>`，`title`、`author`、`content`（HTML）、`created`、`updated`、`titleImage`、`column.{id,title}`；
//! - 想法：`pins.<id>`，`author`、`content`（一组块：`text` 带 HTML、`image` 带 `originalUrl`、`link` 带 `url`、`title`）或 `contentHtml`、
//!   `created`、`updated`。
//!
//! 挑战页（`<meta id="zh-zse-ck">` 加 `zse-ck` 脚本，没有内嵌数据）是量到的：回 `blocked`，不去算、不绕。

use std::collections::BTreeMap;

use scraper::{Html, Selector};
use serde_json::{Map, Value};
use usuyo_plugin_kit::protocol::{CallFailure, ErrorCode, NotFoundReason};

use crate::api::{ApiError, blocked};
use crate::link::{Kind, Target};

/// 这一页的作者。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub name: String,
    /// 页面给的内部 id。
    pub id: Option<String>,
    /// 个人主页的路径名（`/people/<url_token>`）。
    pub url_token: Option<String>,
    pub headline: Option<String>,
    pub organization: bool,
}

impl Person {
    /// 个人主页链接：只从 `url_token` 拼，不信页面给的 `url`。
    pub fn profile_url(&self) -> Option<String> {
        let token = self.url_token.as_deref()?;
        let section = if self.organization { "org" } else { "people" };
        Some(format!("https://www.zhihu.com/{section}/{token}"))
    }

    /// 作者栏的平台 id：主页路径名，没有就是内部 id。
    pub fn platform_id(&self) -> Option<String> {
        self.url_token.clone().or_else(|| self.id.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub id: Option<String>,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct Post {
    pub kind: Kind,
    pub id: String,
    pub title: Option<String>,
    pub question: Option<Question>,
    pub author: Option<Person>,
    /// 文章所在的专栏 `(id, 名字)`。
    pub column: Option<(Option<String>, String)>,
    /// 创建、最后编辑，Unix 秒。
    pub created: Option<i64>,
    pub updated: Option<i64>,
    /// 正文的 HTML。
    pub body: Option<String>,
    /// 页面给的摘要，正文没有时用。
    pub excerpt: Option<String>,
    /// 文章的题图。
    pub cover: Option<String>,
    /// 页面数据里所有名字以 `Count` 结尾的整数。
    pub counts: BTreeMap<String, i64>,
    /// 正文被截断（要登录才给全文）时的原因。
    pub truncated: Option<String>,
    /// 页面数据里有视频。
    pub has_video: bool,
}

#[derive(Debug)]
pub enum PageError {
    /// 内容没有了、藏起来了、在审核：页面这样说。
    Unavailable {
        reason: Option<NotFoundReason>,
        message: String,
        hint: Option<String>,
        detail: String,
    },
    /// 挑战页或验证页。
    Verification { detail: String },
    /// 既没有要的数据，也不是认得的提示页。
    Missing { what: String, detail: String },
}

impl From<PageError> for CallFailure {
    fn from(err: PageError) -> CallFailure {
        match err {
            PageError::Unavailable {
                reason,
                message,
                hint,
                detail,
            } => CallFailure {
                reason,
                hint,
                ..CallFailure::new(ErrorCode::NotFound, message)
            }
            .with_detail(&detail),
            PageError::Verification { detail } => {
                blocked("the page is a verification or challenge page".into()).with_detail(&detail)
            }
            PageError::Missing { what, detail } => CallFailure {
                hint: Some("知乎页面的结构变了，这个插件需要更新".into()),
                ..CallFailure::new(ErrorCode::UpstreamChanged, what)
            }
            .with_detail(&detail),
        }
    }
}

/// 请求没成功时：404、410 的页面是「内容没有了」，带着页面上的话分原因；别的状态照 [`ApiError`] 的对应。
pub fn from_request_error(err: ApiError) -> CallFailure {
    if let ApiError::Answered { error, answer } = &err
        && let ApiError::Http { status, .. } = error.as_ref()
    {
        if matches!(status, 404 | 410) {
            return CallFailure::from(unavailable(
                &answer_text(answer),
                answer,
                if *status == 410 {
                    Some(NotFoundReason::Deleted)
                } else {
                    None
                },
            ));
        }
        if matches!(status, 403 | 401) && is_challenge(answer) {
            return CallFailure::from(PageError::Verification {
                detail: answer.clone(),
            });
        }
    }
    err.into()
}

pub fn parse(html: &str, target: &Target) -> Result<Post, PageError> {
    let doc = Html::parse_document(html);
    let selector = Selector::parse("script#js-initialData").expect("constant selector");
    let Some(script) = doc.select(&selector).next() else {
        return Err(if is_challenge(html) {
            PageError::Verification {
                detail: html.to_owned(),
            }
        } else {
            unavailable_or_missing(&doc, html)
        });
    };
    let raw: String = script.text().collect();
    let state: Value = serde_json::from_str(&raw).map_err(|e| PageError::Missing {
        what: format!("the embedded data is not JSON: {e}"),
        detail: visible_text(&doc),
    })?;
    let entities = state
        .pointer("/initialState/entities")
        .and_then(Value::as_object)
        .ok_or_else(|| PageError::Missing {
            what: "the embedded data has no `initialState.entities`".into(),
            detail: visible_text(&doc),
        })?;
    let table = match target.kind {
        Kind::Answer => "answers",
        Kind::Article => "articles",
        Kind::Pin => "pins",
    };
    let Some(entity) = entities
        .get(table)
        .and_then(|t| t.get(&target.id))
        .and_then(Value::as_object)
    else {
        return Err(unavailable_or_missing_entity(&doc, html, target));
    };
    Ok(post(target, entity, entities))
}

fn post(target: &Target, entity: &Map<String, Value>, entities: &Map<String, Value>) -> Post {
    let question = (target.kind == Kind::Answer)
        .then(|| question(entity, entities, target))
        .flatten();
    let title = match target.kind {
        Kind::Answer => question.as_ref().map(|q| q.title.clone()),
        Kind::Article => text(entity, "title").map(str::to_owned),
        Kind::Pin => None,
    };
    let (body, has_video) = match target.kind {
        Kind::Pin => pin_body(entity),
        _ => (text(entity, "content").map(str::to_owned), false),
    };
    let truncated = ["contentNeedTruncated", "isTruncated", "needTruncate"]
        .iter()
        .any(|k| entity.get(*k).and_then(Value::as_bool) == Some(true))
        .then(|| "the page marks the body as cut".to_owned())
        .or_else(|| {
            body.as_deref()
                .filter(|b| b.contains("登录后"))
                .map(|_| "the body asks the reader to sign in for the full text".to_owned())
        });
    let counts = entity
        .iter()
        .filter(|(k, _)| k.ends_with("Count"))
        .filter_map(|(k, v)| Some((k.clone(), v.as_i64()?)))
        .collect();
    Post {
        kind: target.kind,
        id: target.id.clone(),
        title,
        question,
        author: entity
            .get("author")
            .and_then(Value::as_object)
            .and_then(person),
        column: entity
            .get("column")
            .and_then(Value::as_object)
            .and_then(|c| Some((id_text(c, "id"), text(c, "title")?.to_owned()))),
        created: int(entity, &["createdTime", "created"]),
        updated: int(entity, &["updatedTime", "updated"]),
        body,
        excerpt: text(entity, "excerpt").map(str::to_owned),
        cover: text(entity, "titleImage")
            .or_else(|| text(entity, "imageUrl"))
            .filter(|c| !c.is_empty())
            .map(str::to_owned),
        counts,
        truncated,
        has_video,
    }
}

fn question(
    answer: &Map<String, Value>,
    entities: &Map<String, Value>,
    target: &Target,
) -> Option<Question> {
    let inline = answer.get("question").and_then(Value::as_object);
    let id = inline
        .and_then(|q| id_text(q, "id"))
        .or_else(|| target.question.clone());
    let from_table = id
        .as_deref()
        .and_then(|id| entities.get("questions")?.get(id)?.as_object());
    let title = inline
        .and_then(|q| text(q, "title"))
        .or_else(|| from_table.and_then(|q| text(q, "title")))?;
    Some(Question {
        id,
        title: title.to_owned(),
    })
}

fn person(author: &Map<String, Value>) -> Option<Person> {
    let name = text(author, "name")?.to_owned();
    let token = |key| {
        text(author, key)
            .filter(|t| {
                !t.is_empty()
                    && t.chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            })
            .map(str::to_owned)
    };
    Some(Person {
        name,
        id: id_text(author, "id").filter(|i| !i.is_empty()),
        url_token: token("urlToken"),
        headline: text(author, "headline")
            .filter(|h| !h.is_empty())
            .map(str::to_owned),
        organization: text(author, "userType") == Some("organization"),
    })
}

/// 想法的正文：`content` 是一组块；没有时用 `contentHtml`。返回 HTML 和有没有视频。
fn pin_body(pin: &Map<String, Value>) -> (Option<String>, bool) {
    let Some(blocks) = pin.get("content").and_then(Value::as_array) else {
        return (text(pin, "contentHtml").map(str::to_owned), false);
    };
    let mut html = String::new();
    let mut video = false;
    for block in blocks.iter().filter_map(Value::as_object) {
        match text(block, "type") {
            Some("text") => {
                if let Some(t) = text(block, "content").or_else(|| text(block, "ownText")) {
                    html.push_str(&format!("<div>{t}</div>"));
                }
            }
            Some("image") => {
                if let Some(u) = text(block, "originalUrl").or_else(|| text(block, "url")) {
                    html.push_str(&format!("<p><img data-original=\"{}\"></p>", attr(u)));
                }
            }
            Some("link") => {
                if let Some(u) = text(block, "url") {
                    let title = text(block, "title").unwrap_or(u);
                    html.push_str(&format!(
                        "<p><a href=\"{}\">{}</a></p>",
                        attr(u),
                        element_text(title)
                    ));
                }
            }
            Some("video") => video = true,
            _ => {}
        }
    }
    ((!html.is_empty()).then_some(html), video)
}

fn attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn element_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn text<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key)?.as_str()
}

/// 数字或字符串写的 id。
fn id_text(map: &Map<String, Value>, key: &str) -> Option<String> {
    match map.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn int(map: &Map<String, Value>, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|k| map.get(*k)?.as_i64())
        .filter(|t| *t > 0)
}

/// 挑战页：`<meta id="zh-zse-ck">`（量到的），或接口回的 40362 一类验证提示、`unhuman` 页。
fn is_challenge(body: &str) -> bool {
    body.contains("zh-zse-ck")
        || body.contains("/zse-ck/")
        || body.contains("account/unhuman")
        || body.contains("\"code\":40362")
}

fn visible_text(doc: &Html) -> String {
    doc.root_element()
        .descendants()
        .filter_map(|n| match n.value() {
            scraper::Node::Text(t) => Some(t.trim().to_owned()),
            _ => None,
        })
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn answer_text(body: &str) -> String {
    visible_text(&Html::parse_document(body))
}

fn unavailable_or_missing(doc: &Html, html: &str) -> PageError {
    let visible = visible_text(doc);
    match cue(&visible) {
        Some(_) => unavailable(&visible, html, None),
        None => PageError::Missing {
            what: "the page has no embedded data and is not a known notice page".into(),
            detail: visible,
        },
    }
}

fn unavailable_or_missing_entity(doc: &Html, html: &str, target: &Target) -> PageError {
    let visible = visible_text(doc);
    // 安全验证页带着内嵌数据、里面没有要的那一条，标题是「安全验证 - 知乎」（2026-11-16 量到的）
    if visible.trim_start().starts_with("安全验证") {
        return PageError::Verification {
            detail: html.to_owned(),
        };
    }
    let mut err = unavailable(&visible, html, None);
    if cue(&visible).is_none()
        && let PageError::Unavailable { message, .. } = &mut err
    {
        *message = format!(
            "the page has no {} `{}` in its data",
            target.kind.name(),
            target.id
        );
    }
    err
}

/// 页面上的话说明为什么看不到：返回原因和一句英文。
fn cue(visible: &str) -> Option<(NotFoundReason, &'static str)> {
    const DELETED: [&str; 4] = ["已被删除", "内容已删除", "已经被删除", "没有知识存在的荒原"];
    const REVIEW: [&str; 2] = ["审核中", "正在审核"];
    const HIDDEN: [&str; 6] = [
        "已被隐藏",
        "仅自己可见",
        "已被折叠",
        "违反",
        "已被封禁",
        "暂时无法查看",
    ];
    let has = |list: &[&str]| list.iter().any(|w| visible.contains(w));
    if has(&DELETED) {
        Some((NotFoundReason::Deleted, "deleted"))
    } else if has(&REVIEW) {
        Some((NotFoundReason::Private, "pending review"))
    } else if has(&HIDDEN) {
        Some((NotFoundReason::Private, "hidden"))
    } else {
        None
    }
}

fn unavailable(visible: &str, raw: &str, fallback: Option<NotFoundReason>) -> PageError {
    let found = cue(visible);
    let reason = found.map(|(r, _)| r).or(fallback);
    let (message, hint) = match found {
        Some((_, why)) => (
            format!("the content is not available: {why}"),
            Some(match why {
                "deleted" => "这条内容已被删除",
                "pending review" => "这条内容还在审核中，暂时看不到",
                _ => "这条内容被隐藏了，现在看不到",
            }),
        ),
        None => (
            "the page says the content does not exist".to_owned(),
            Some("知乎上找不到这条内容"),
        ),
    };
    PageError::Unavailable {
        reason,
        message,
        hint: hint.map(str::to_owned),
        detail: if visible.is_empty() { raw } else { visible }.to_owned(),
    }
}
