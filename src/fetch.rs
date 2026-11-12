//! 方法 `page`：一条回答、文章或想法的链接取一条内容（协议 §3.6、§3.7）。不带账号：请求都不带 cookie，不读宿主交来的凭据，
//! 不带任何客户端算出来的参数。
//!
//! 交出的东西：
//! - `text`：正文的 Markdown（`textFormat: markdown`，写法和限制见 `markdown.rs`）。图片的位置写页面给的原地址；
//! - `html`：`page.html`，取到的页面原样；
//! - `images`：文章的题图 `images/cover.<ext>`，再是正文里的图片 `images/001.<ext>`…，按第一次出现的次序，原图（`data-original`），
//!   同一个地址只存一份；对应关系在 `meta.json`；
//! - `title`：回答是问题的标题，文章是文章的标题，想法没有；
//! - `meta.json`：问题、作者（名字、内部 id、主页路径名、主页链接）、创建和最后编辑的时间、各种计数、专栏、图片的地址与文件。
//!
//! 不下载页面里的视频：视频卡片只把地址写进正文的链接、记进 `meta.json`，并在 `missing` 里写 `media:video`，宿主据此找能补的方法
//! （知乎视频页 `/zvideo/` 由 yt-dlp 插件取）。

use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, Instant};

use scraper::Html;
use serde_json::{Value, json};
use usuyo_plugin_kit::protocol::{
    Access, Author, CallFailure, Completeness, ErrorCode, Evidence, FetchOutput, ItemKind,
    OutputKind, RelPath, Rfc3339, TextFormat,
};

use usuyo_plugin_kit::http::Cookie;

use crate::api::{ApiError, Zhihu, progress, visitor_missing, visitor_rejected};
use crate::link::{self, Kind, Target};
use crate::markdown::{self, Converted};
use crate::page::{self, Post};

/// 本方法能给的输出，和清单里的 `outputs` 一致。
pub const OUTPUTS: [OutputKind; 4] = [
    OutputKind::Text,
    OutputKind::Html,
    OutputKind::Images,
    OutputKind::Meta,
];

const IMAGE_BYTES: u64 = 40 << 20;
const IMAGES_TOTAL_BYTES: u64 = 1 << 30;
/// 图片下载的总时间预算；超了的不再下，记进 `missing`。
const IMAGES_BUDGET: Duration = Duration::from_secs(8 * 60);
/// 一篇最多存几张图：图片分享最多 20 张，长文几十张，再多的不取。
const MAX_IMAGES: usize = 300;
const TITLE_CHARS: usize = 100;

pub fn fetch(
    api: &Zhihu,
    cookie: Option<&Cookie>,
    input: &str,
    out: &Path,
    want: Option<&[OutputKind]>,
) -> Result<FetchOutput, CallFailure> {
    let wanted: BTreeSet<OutputKind> =
        want.map_or_else(|| OUTPUTS.into(), |w| w.iter().copied().collect());
    if let Some(other) = wanted.iter().find(|w| !OUTPUTS.contains(w)) {
        return Err(CallFailure::new(
            ErrorCode::InvalidArgs,
            format!("cannot give `{other}`"),
        ));
    }
    let target = link::parse_input(input).ok_or_else(|| {
        CallFailure::new(
            ErrorCode::UnsupportedInput,
            "not a zhihu answer, article or pin link",
        )
    })?;
    let identity = target.identity()?;

    let fetched_at = Rfc3339::from_timestamp(jiff::Timestamp::now());
    // 没有访客身份：不发请求
    let cookie = cookie.ok_or_else(visitor_missing)?;
    let fetched = api
        .page(&target.request_url(), cookie)
        .map_err(|e| rejected(page::from_request_error(e)))?;
    let html = String::from_utf8_lossy(&fetched.body);
    let post = page::parse(&html, &target).map_err(|e| rejected(CallFailure::from(e)))?;

    let mut output = FetchOutput {
        identity: Some(identity),
        kind: match post.kind {
            Kind::Answer | Kind::Article => ItemKind::Article,
            Kind::Pin => ItemKind::Post,
        },
        title: None,
        author: None,
        published_at: None,
        text: None,
        text_format: None,
        html: None,
        images: Vec::new(),
        media: Vec::new(),
        platform_tags: Vec::new(),
        comments: None,
        completeness: Completeness::Partial,
        missing: Vec::new(),
        missing_reason: None,
        evidence: Evidence {
            fetched_at,
            access: Access::PublicWeb,
            site: None,
            upstream: None,
            status: Some(fetched.status),
        },
        credentials_update: None,
    };
    let mut reasons: Vec<String> = Vec::new();
    // 这一篇本来没有的部分不算缺（协议 §3.7）；交来一部分、还缺一部分的照样记缺
    let mut absent: BTreeSet<OutputKind> = BTreeSet::new();
    let mut partial: BTreeSet<OutputKind> = BTreeSet::new();

    let converted = convert(api, &post);

    if wanted.contains(&OutputKind::Html) {
        write(out, "page.html", &fetched.body)?;
        output.html = Some(rel("page.html")?);
    }

    let text = text_of(&post, &converted);
    if text.is_none() {
        absent.insert(OutputKind::Text);
    } else if wanted.contains(&OutputKind::Text) {
        output.text = text;
        output.text_format = Some(TextFormat::Markdown);
        if let Some(why) = &post.truncated {
            partial.insert(OutputKind::Text);
            reasons.push(format!("正文被截断：{why}"));
        }
    }

    // 页面里的图片：封面在前，正文里的按次序；封面和正文某张同一个地址时只存一份
    let mut sources: Vec<(String, String)> = Vec::new();
    if let Some(cover) = post
        .cover
        .as_deref()
        .filter(|c| api.endpoints().is_article_image(c))
        && !converted.images.iter().any(|i| i == cover)
    {
        sources.push(("cover".into(), cover.to_owned()));
    }
    for (i, url) in converted.images.iter().enumerate().take(MAX_IMAGES) {
        sources.push((format!("{:03}", i + 1), url.clone()));
    }
    let over = converted.images.len().saturating_sub(MAX_IMAGES);
    let mut records: Vec<Value> = Vec::new();
    if sources.is_empty() {
        absent.insert(OutputKind::Images);
    } else if wanted.contains(&OutputKind::Images) {
        let failed = download_images(api, out, &sources, &mut output, &mut records)?;
        let total = sources.len() + over;
        let lost = failed + over;
        if lost > 0 {
            partial.insert(OutputKind::Images);
            reasons.push(if output.images.is_empty() {
                format!("图片都没有下载下来（共 {total} 张）")
            } else {
                format!("{lost} 张图片没有下载下来（共 {total} 张）")
            });
        }
    }

    // 视频：只记地址，不下载
    let mut extra_missing: Vec<OutputKind> = Vec::new();
    if post.has_video || !converted.videos.is_empty() {
        extra_missing.push(OutputKind::Video);
        reasons.push("内容里有视频：这种方法只记下页面给的地址，不下载".into());
    }

    let meta_wanted = wanted.contains(&OutputKind::Meta);
    if meta_wanted {
        output.title = title_of(&post);
        output.author = author_of(&post);
        output.published_at = post
            .created
            .and_then(|t| jiff::Timestamp::from_second(t).ok())
            .map(Rfc3339::from_timestamp);
        if output.title.is_none() && output.author.is_none() && output.published_at.is_none() {
            absent.insert(OutputKind::Meta);
        }
        write(
            out,
            "meta.json",
            &serde_json::to_vec_pretty(&meta(&target, &post, &converted, &records, &output))
                .map_err(|e| CallFailure::new(ErrorCode::Internal, format!("meta.json: {e}")))?,
        )?;
    }

    let obtained = output.obtained();
    output.missing = wanted
        .iter()
        .filter(|kind| {
            !absent.contains(kind) && (partial.contains(kind) || !obtained.contains(kind))
        })
        .copied()
        .chain(extra_missing)
        .collect();
    output.completeness = if output.missing.is_empty() {
        Completeness::Full
    } else {
        Completeness::Partial
    };
    if !output.missing.is_empty() {
        output.missing_reason = Some(reasons.join("；"));
    }
    Ok(output)
}

/// 带着访客身份还遇到挑战页或 403：身份不再被收。
fn rejected(failure: CallFailure) -> CallFailure {
    if failure.code == ErrorCode::Blocked {
        visitor_rejected(failure)
    } else {
        failure
    }
}

fn convert(api: &Zhihu, post: &Post) -> Converted {
    let is_image = |u: &str| api.endpoints().is_article_image(u);
    match &post.body {
        Some(html) => {
            let doc = Html::parse_fragment(html);
            markdown::convert(doc.root_element(), is_image)
        }
        None => Converted::default(),
    }
}

/// 正文；没有正文时用页面给的摘要。
fn text_of(post: &Post, converted: &Converted) -> Option<String> {
    let text = converted.markdown.trim();
    if !text.is_empty() {
        return Some(text.to_owned());
    }
    let excerpt = post.excerpt.as_deref()?;
    let text = markdown::convert_text(excerpt, |_| false).markdown;
    (!text.trim().is_empty()).then_some(text)
}

fn title_of(post: &Post) -> Option<String> {
    let line = post
        .title
        .as_deref()?
        .lines()
        .find(|l| !l.trim().is_empty())?;
    Some(line.trim().chars().take(TITLE_CHARS).collect())
}

fn author_of(post: &Post) -> Option<Author> {
    let person = post.author.as_ref()?;
    Some(Author {
        name: person.name.clone(),
        platform_id: person.platform_id(),
        url: person.profile_url(),
    })
}

/// 下载并存好图片；返回没存下来的张数。限流是硬错误，别的只记进 `records`。
fn download_images(
    api: &Zhihu,
    out: &Path,
    sources: &[(String, String)],
    output: &mut FetchOutput,
    records: &mut Vec<Value>,
) -> Result<usize, CallFailure> {
    let started = Instant::now();
    let mut total_bytes = 0u64;
    let mut failed = 0usize;
    for (done, (stem, raw)) in sources.iter().enumerate() {
        progress(done, sources.len(), "图片");
        let mut record = json!({"name": stem, "url": raw});
        let saved = if started.elapsed() > IMAGES_BUDGET {
            Err("超出下载图片的时间预算".to_owned())
        } else if total_bytes > IMAGES_TOTAL_BYTES {
            Err("超出图片的总大小上限".to_owned())
        } else {
            match fetch_image(api, stem, raw) {
                Ok((name, bytes)) => {
                    // 写盘失败不是图片的问题，整次失败
                    write(out, &name, &bytes)?;
                    total_bytes += bytes.len() as u64;
                    Ok(rel(&name)?)
                }
                Err(e) if e.is_hard() => return Err(e.into()),
                Err(e) => Err(e.to_string()),
            }
        };
        match saved {
            Ok(path) => {
                record["path"] = path.as_str().into();
                output.images.push(path);
            }
            Err(why) => {
                failed += 1;
                record["why"] = why.into();
            }
        }
        records.push(record);
    }
    Ok(failed)
}

/// 下载一张图，返回要存的文件名（输出目录里的相对路径）和内容。
fn fetch_image(api: &Zhihu, stem: &str, raw: &str) -> Result<(String, Vec<u8>), ApiError> {
    let url = api.endpoints().image_url(raw)?;
    let (bytes, content_type) = api.image(&url, IMAGE_BYTES)?;
    let name = format!("images/{stem}.{}", extension(content_type.as_deref(), raw));
    Ok((name, bytes))
}

/// 文件扩展名：先看响应的 `Content-Type`，再看地址路径的扩展名，都没有写 `jpg`。
fn extension(content_type: Option<&str>, url: &str) -> &'static str {
    let by_mime = content_type
        .and_then(|m| m.split(';').next())
        .map(|m| m.trim().to_ascii_lowercase())
        .and_then(|m| mime_extension(m.strip_prefix("image/")?));
    let by_path = url::Url::parse(url).ok().and_then(|u| {
        let name = u.path_segments()?.next_back()?.to_owned();
        let (_, ext) = name.rsplit_once('.')?;
        mime_extension(&ext.to_ascii_lowercase())
    });
    by_mime.or(by_path).unwrap_or("jpg")
}

fn mime_extension(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "jpeg" | "jpg" => "jpg",
        "png" => "png",
        "gif" => "gif",
        "webp" => "webp",
        "avif" => "avif",
        "bmp" => "bmp",
        "svg" | "svg+xml" => "svg",
        _ => return None,
    })
}

fn meta(
    target: &Target,
    post: &Post,
    converted: &Converted,
    images: &[Value],
    output: &FetchOutput,
) -> Value {
    let at = |t: Option<i64>| {
        t.and_then(|t| jiff::Timestamp::from_second(t).ok())
            .map(Rfc3339::from_timestamp)
    };
    json!({
        "kind": target.kind.name(),
        "id": target.id,
        "url": target.canonical_url(),
        "title": output.title,
        "question": post.question.as_ref().map(|q| json!({
            "id": q.id,
            "title": q.title,
            "url": q.id.as_ref().map(|id| format!("https://www.zhihu.com/question/{id}")),
        })),
        "author": post.author.as_ref().map(|p| json!({
            "name": p.name,
            "id": p.id,
            "urlToken": p.url_token,
            "url": p.profile_url(),
            "headline": p.headline,
            "organization": p.organization,
        })),
        "column": post.column.as_ref().map(|(id, title)| json!({"id": id, "title": title})),
        "createdAt": at(post.created),
        "updatedAt": at(post.updated),
        "counts": post.counts,
        "truncated": post.truncated,
        "images": images,
        "embeds": {
            "videos": converted.videos,
            "frames": converted.embeds,
            "pageVideo": post.has_video,
        },
    })
}

fn rel(path: &str) -> Result<RelPath, CallFailure> {
    RelPath::try_from(path).map_err(|e| CallFailure::new(ErrorCode::Internal, e))
}

fn write(out: &Path, name: &str, bytes: &[u8]) -> Result<(), CallFailure> {
    let dest = out.join(name);
    let io = |e: std::io::Error| {
        CallFailure::new(ErrorCode::Internal, format!("cannot write `{name}`: {e}"))
    };
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    std::fs::write(&dest, bytes).map_err(io)
}
