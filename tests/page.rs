//! 对着本机的假服务器跑（没有对着真实的知乎测过；访客身份是编的 cookie）：没有访客身份不发请求、挑战页算身份被拒、回答（带问题标题、公式、代码、链接卡片、视频卡片、原图）、文章（题图、专栏、机构作者）、想法（块）、
//! 被截断的回答、匿名回答、被删除、审核中、隐藏、挑战页、HTTP 状态和图片下载失败。不碰真网络。
//!
//! `fixtures/` 里的页面都是手写的，内容是编的。外层结构照 2026-11-12 量到的公开收藏夹页写（`<script id="js-initialData"
//! type="text/json">`，`initialState.entities` 下按类别存）；`challenge.html` 的形状是量到的（`<meta id="zh-zse-ck">` 加一个
//! `zse-ck` 脚本，650 字节）。回答、文章、想法条目里的字段名没有对着真实页面量过。

mod support;

use std::net::IpAddr;
use std::path::Path;

use support::{Request, Response, Server, fixture};
use usuyo_fetch_zhihu::api::{Endpoints, Zhihu};
use usuyo_fetch_zhihu::fetch;
use usuyo_plugin::protocol::{
    Access, CallFailure, Completeness, ErrorCode, FetchOutput, Identity, ItemKind, NotFoundReason,
    OutputKind, TextFormat,
};
use usuyo_plugin_kit::http::{Cookie, Policy};

const VISITOR: &str = "KLC_VER=1; d_c0=sample-visitor-cookie";

fn local_policy() -> Policy {
    Policy {
        lookup: |_, _| {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no DNS in tests",
            ))
        },
        public: |ip| ip == IpAddr::from([127, 0, 0, 1]),
        cookies_over_http: true,
        ..Policy::strict()
    }
}

fn html(name: &str, base: &str) -> Response {
    page(200, name, base)
}

fn page(status: u16, name: &str, base: &str) -> Response {
    Response {
        status,
        headers: vec![("Content-Type", "text/html; charset=utf-8".into())],
        body: fixture(name, base).into_bytes(),
    }
}

fn image(content_type: Option<&str>, bytes: &[u8]) -> Response {
    Response {
        status: 200,
        headers: content_type
            .map(|t| vec![("Content-Type", t.to_owned())])
            .unwrap_or_default(),
        body: bytes.to_vec(),
    }
}

fn answer_page(id: &str, base: &str) -> Response {
    match id {
        "1234567890" => html("answer.html", base),
        "1234567891" => html("answer_truncated.html", base),
        "1234567892" => html("answer_anonymous.html", base),
        "1234567893" => html("answer_question_table.html", base),
        "2000000001" => page(403, "challenge.html", base),
        "2000000002" => html("challenge.html", base),
        "2000000003" => html("gone.html", base),
        "2000000004" => html("review.html", base),
        "2000000005" => html("hidden.html", base),
        "2000000006" => html("unknown.html", base),
        "2000000007" => html("no_data.html", base),
        "2000000008" => html("broken_data.html", base),
        "2000000009" => page(404, "gone.html", base),
        "2000000010" => page(410, "no_data.html", base),
        "2000000011" => Response::status(429).header("Retry-After", "30"),
        "2000000012" => Response::status(500),
        "2000000013" => Response::redirect("/account/unhuman?type=Q"),
        "2000000015" => Response::redirect("https://example.com/x"),
        "2000000016" => Response::status(401),
        "2000000018" => Response::status(300).header(
            "Location",
            "https://www.zhihu.com/signin?next=%2Fquestion%2F1",
        ),
        "2000000019" => Response::status(300).header("Location", "/login"),
        // 同主机的 302：插件会跟，落在登录页的 200
        "2000000020" => Response::redirect("/signin?next=%2Fquestion%2F1"),
        // 图片没有的回答：把页面里一张图换成不存在的
        "2000000017" => Response {
            status: 200,
            headers: vec![("Content-Type", "text/html; charset=utf-8".into())],
            body: fixture("answer.html", base)
                .replace("v2-bbbb222", "v2-missing0")
                .replace("1234567890", "2000000017")
                .into_bytes(),
        },
        _ => Response::status(404),
    }
}

/// 一个什么都有的知乎。
fn zhihu(req: &Request, base: &str) -> Response {
    let path = req.path.as_str();
    if let Some(id) = path.strip_prefix("/answer/") {
        return answer_page(id, base);
    }
    if let Some(rest) = path.strip_prefix("/question/") {
        // 问题下的写法：回答 id 在最后一段
        return match rest.split('/').collect::<Vec<_>>().as_slice() {
            [_, "answer", id] => answer_page(id, base),
            _ => Response::status(404),
        };
    }
    match path {
        "/account/unhuman" => html("challenge.html", base),
        "/signin" => Response::status(200),
        "/p/19970400" => html("article.html", base),
        "/pin/1500000000000000001" => html("pin.html", base),
        "/pin/1500000000000000002" => html("pin_video.html", base),
        "/v2-aaaa111_r.jpg" => image(Some("image/jpeg"), b"IMG-A"),
        "/v2-bbbb222_r.png" => image(Some("image/png"), b"IMG-B"),
        "/v2-cccc333_r.jpg" => image(None, b"IMG-C"),
        "/v2-cover999_r.jpg" => image(Some("image/jpeg"), b"COVER"),
        "/v2-pppp444_r.jpg" => image(Some("image/webp"), b"PIN-IMG"),
        _ => Response::status(404),
    }
}

fn api_for(server: &Server) -> Zhihu {
    Zhihu::new(Endpoints::local(&server.base, local_policy()))
}

fn failure<T: std::fmt::Debug>(result: Result<T, CallFailure>) -> CallFailure {
    result.expect_err("expected a failure")
}

fn answer_url(id: &str) -> String {
    format!("https://www.zhihu.com/answer/{id}")
}

fn visitor() -> Cookie {
    Cookie::new(VISITOR, ["127.0.0.1"])
}

fn fetch_with(
    server: &Server,
    input: &str,
    want: Option<&[OutputKind]>,
) -> (tempfile::TempDir, Result<FetchOutput, CallFailure>) {
    let tmp = tempfile::tempdir().unwrap();
    let result = fetch::fetch(&api_for(server), Some(&visitor()), input, tmp.path(), want);
    (tmp, result)
}

fn fetch_ok(server: &Server, input: &str) -> (tempfile::TempDir, FetchOutput) {
    let (tmp, result) = fetch_with(server, input, None);
    (tmp, result.unwrap())
}

/// 输出目录里的全部文件，相对路径，排好序。
fn files(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                out.push(
                    path.strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn image_paths(out: &FetchOutput) -> Vec<&str> {
    out.images.iter().map(|p| p.as_str()).collect()
}

fn content_id(out: &FetchOutput) -> &str {
    match &out.identity {
        Some(Identity::Platform {
            platform,
            content_id,
            ..
        }) => {
            assert_eq!(platform.as_str(), "zhihu");
            content_id
        }
        other => panic!("{other:?}"),
    }
}

fn meta(dir: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(dir.join("meta.json")).unwrap()).unwrap()
}

fn text(out: &FetchOutput) -> &str {
    out.text.as_deref().unwrap()
}

#[test]
fn an_answer_gives_the_question_markdown_original_images_the_page_and_meta() {
    let server = Server::start(zhihu);
    let (tmp, out) = fetch_ok(
        &server,
        "https://www.zhihu.com/question/19550225/answer/1234567890?utm_source=wechat",
    );

    assert_eq!(content_id(&out), "answer:1234567890");
    assert_eq!(out.kind, ItemKind::Article);
    assert_eq!(out.title.as_deref(), Some("示例问题：这是一个问题吗？"));
    let author = out.author.as_ref().unwrap();
    assert_eq!(
        (
            author.name.as_str(),
            author.platform_id.as_deref(),
            author.url.as_deref()
        ),
        (
            "示例作者",
            Some("sample-author"),
            Some("https://www.zhihu.com/people/sample-author")
        )
    );
    assert_eq!(
        out.published_at.as_ref().unwrap().as_str(),
        "2023-11-14T22:13:20Z"
    );
    assert_eq!(out.evidence.access, Access::PublicWeb);

    assert_eq!(out.text_format, Some(TextFormat::Markdown));
    let base = &server.base;
    assert_eq!(
        text(&out),
        format!(
            "先说结论：**可以**。设 $x^2 + y_1 = \\frac{{a}}{{b}}$ 成立。\n\n\
             ## 步骤\n\n\
             1. 第一步\n2. 第二步 ``a`b``\n\n\
             > 引用一句\n\n\
             ```python\nprint('hi')\nif a < 2:\n    pass\n```\n\n\
             ![]({base}/v2-aaaa111_r.jpg)\n\n图一\n\n\
             参考 [示例参考](https://example.com/ref) 和 [这个](https://example.org/p)。\n\n\
             ![]({base}/v2-bbbb222_r.png)\n\n\
             [一段演示视频](https://www.zhihu.com/video/1111111111111111111)"
        )
    );

    // 图片：按次序、原图；缩略图（_720w）一张都没请求；视频卡片的封面不下载（卡片只写链接）
    assert_eq!(image_paths(&out), ["images/001.jpg", "images/002.png"]);
    assert_eq!(
        std::fs::read(tmp.path().join("images/001.jpg")).unwrap(),
        b"IMG-A"
    );
    assert_eq!(
        std::fs::read(tmp.path().join("images/002.png")).unwrap(),
        b"IMG-B"
    );
    assert!(
        server.paths().iter().all(|p| !p.contains("720w")),
        "{:?}",
        server.paths()
    );
    assert_eq!(
        files(tmp.path()),
        ["images/001.jpg", "images/002.png", "meta.json", "page.html"]
    );
    assert_eq!(out.html.as_ref().unwrap().as_str(), "page.html");
    assert!(
        std::fs::read_to_string(tmp.path().join("page.html"))
            .unwrap()
            .contains("js-initialData")
    );

    // 视频只记地址，不下载：缺 `media:video`
    assert_eq!(out.completeness, Completeness::Partial);
    assert_eq!(out.missing, [OutputKind::Video]);
    assert!(out.missing_reason.as_deref().unwrap().contains("视频"));

    let m = meta(tmp.path());
    assert_eq!(m["kind"], "answer");
    assert_eq!(m["id"], "1234567890");
    assert_eq!(m["url"], "https://www.zhihu.com/answer/1234567890");
    assert_eq!(m["question"]["id"], "19550225");
    assert_eq!(m["question"]["title"], "示例问题：这是一个问题吗？");
    assert_eq!(
        m["question"]["url"],
        "https://www.zhihu.com/question/19550225"
    );
    assert_eq!(m["author"]["name"], "示例作者");
    assert_eq!(m["author"]["id"], "a1b2c3d4e5f60718293a4b5c6d7e8f90");
    assert_eq!(m["author"]["urlToken"], "sample-author");
    assert_eq!(m["author"]["headline"], "一句话介绍");
    assert_eq!(m["createdAt"], "2023-11-14T22:13:20Z");
    assert_eq!(m["updatedAt"], "2023-11-14T23:13:20Z");
    assert_eq!(m["counts"]["voteupCount"], 1234);
    assert_eq!(m["counts"]["commentCount"], 56);
    assert_eq!(m["counts"]["thanksCount"], 7);
    assert_eq!(m["counts"]["favlistsCount"], 89);
    assert_eq!(m["images"][0]["name"], "001");
    assert_eq!(m["images"][0]["path"], "images/001.jpg");
    assert_eq!(
        m["embeds"]["videos"][0],
        "https://www.zhihu.com/video/1111111111111111111"
    );

    // 请求：回答用问题下的写法，一次；不带 cookie、不带 Referer
    let reqs = server.requests();
    assert_eq!(reqs[0].path, "/question/19550225/answer/1234567890");
    assert_eq!(reqs[0].query.len(), 0);
    assert!(reqs.iter().all(|r| !r.headers.contains_key("referer")));
    // 访客身份只跟着页面请求；图片请求不带 cookie
    assert_eq!(reqs[0].cookie(), Some(VISITOR));
    assert!(reqs[1..].iter().all(|r| r.cookie().is_none()), "{reqs:?}");
    assert!(reqs[0].headers["user-agent"].contains("Mozilla/5.0"));
}

#[test]
fn the_short_answer_form_is_requested_as_it_is_and_the_identity_is_the_same() {
    let server = Server::start(zhihu);
    let (_tmp, out) = fetch_ok(&server, &answer_url("1234567890"));
    assert_eq!(content_id(&out), "answer:1234567890");
    assert_eq!(server.paths()[0], "/answer/1234567890");
}

#[test]
fn the_question_title_can_come_from_the_question_table() {
    let server = Server::start(zhihu);
    let (_tmp, out) = fetch_ok(&server, &answer_url("1234567893"));
    assert_eq!(out.title.as_deref(), Some("问题表里的标题"));
    assert_eq!(text(&out), "题目在问题表里");
    assert_eq!(out.completeness, Completeness::Full);
}

#[test]
fn an_anonymous_author_has_no_profile() {
    let server = Server::start(zhihu);
    let (tmp, out) = fetch_ok(&server, &answer_url("1234567892"));
    let author = out.author.as_ref().unwrap();
    assert_eq!(author.name, "匿名用户");
    // 内部 id 是 "0"；主页路径名是空的，不拼链接
    assert_eq!(author.platform_id.as_deref(), Some("0"));
    assert_eq!(author.url, None);
    assert_eq!(meta(tmp.path())["author"]["url"], serde_json::Value::Null);
    assert_eq!(out.completeness, Completeness::Full);
    assert!(out.images.is_empty() && out.missing.is_empty());
}

#[test]
fn an_article_has_a_cover_a_column_and_an_organization_author() {
    let server = Server::start(zhihu);
    let (tmp, out) = fetch_ok(&server, "https://zhuanlan.zhihu.com/p/19970400?utm_id=0");
    assert_eq!(content_id(&out), "article:19970400");
    assert_eq!(out.kind, ItemKind::Article);
    assert_eq!(out.title.as_deref(), Some("示例文章：一篇专栏"));
    let author = out.author.as_ref().unwrap();
    assert_eq!(
        author.url.as_deref(),
        Some("https://www.zhihu.com/org/sample-org")
    );
    assert_eq!(
        text(&out),
        format!(
            "文章第一段。\n\n![]({}/v2-cccc333_r.jpg)\n\n```rust\nlet a = 1;\n```",
            server.base
        )
    );
    // 题图在前；没有 Content-Type 的图按地址的扩展名
    assert_eq!(image_paths(&out), ["images/cover.jpg", "images/001.jpg"]);
    assert_eq!(out.completeness, Completeness::Full);
    let m = meta(tmp.path());
    assert_eq!(m["column"]["title"], "示例专栏");
    assert_eq!(m["column"]["id"], "c_100");
    assert_eq!(m["counts"]["voteupCount"], 321);
    assert_eq!(server.paths()[0], "/p/19970400");
}

#[test]
fn a_pin_is_a_post_made_of_its_blocks() {
    let server = Server::start(zhihu);
    let (tmp, out) = fetch_ok(&server, "https://www.zhihu.com/pin/1500000000000000001");
    assert_eq!(content_id(&out), "pin:1500000000000000001");
    assert_eq!(out.kind, ItemKind::Post);
    assert_eq!(out.title, None);
    assert_eq!(
        text(&out),
        format!(
            "一条想法 **带粗体**\n\n![]({}/v2-pppp444_r.jpg)\n\n[示例链接](https://example.com/x)",
            server.base
        )
    );
    assert_eq!(image_paths(&out), ["images/001.webp"]);
    assert_eq!(out.completeness, Completeness::Full);
    assert_eq!(meta(tmp.path())["counts"]["likeCount"], 5);

    let (_tmp, out) = fetch_ok(&server, "https://www.zhihu.com/pin/1500000000000000002");
    assert_eq!(text(&out), "带视频");
    assert_eq!(out.missing, [OutputKind::Video]);
}

#[test]
fn a_cut_body_is_delivered_and_marked_missing_with_the_reason() {
    let server = Server::start(zhihu);
    let (_tmp, out) = fetch_ok(&server, &answer_url("1234567891"));
    assert_eq!(text(&out), "只有前一段");
    assert_eq!(out.completeness, Completeness::Partial);
    assert_eq!(out.missing, [OutputKind::Text]);
    assert!(out.missing_reason.as_deref().unwrap().contains("截断"));
}

#[test]
fn only_what_is_wanted_is_given() {
    let server = Server::start(zhihu);
    let (tmp, result) = fetch_with(
        &server,
        &answer_url("1234567890"),
        Some(&[OutputKind::Text, OutputKind::Meta]),
    );
    let out = result.unwrap();
    assert!(out.html.is_none() && out.images.is_empty());
    assert!(out.text.is_some() && out.title.is_some());
    assert_eq!(files(tmp.path()), ["meta.json"]);
    assert!(
        server
            .paths()
            .iter()
            .all(|p| !p.ends_with(".jpg") && !p.ends_with(".png"))
    );

    let (_tmp, result) = fetch_with(
        &server,
        &answer_url("1234567890"),
        Some(&[OutputKind::Video]),
    );
    assert_eq!(failure(result).code, ErrorCode::InvalidArgs);
}

#[test]
fn a_missing_image_is_noted_and_the_text_is_still_given() {
    let server = Server::start(zhihu);
    let (_tmp, out) = fetch_ok(&server, &answer_url("2000000017"));
    assert_eq!(image_paths(&out), ["images/001.jpg"]);
    assert!(out.text.is_some());
    assert_eq!(out.completeness, Completeness::Partial);
    assert!(out.missing.contains(&OutputKind::Images));
    assert!(out.missing_reason.as_deref().unwrap().contains("1 张图片"));
}

#[test]
fn without_a_visitor_identity_nothing_is_requested() {
    let server = Server::start(zhihu);
    let tmp = tempfile::tempdir().unwrap();
    let f = failure(fetch::fetch(
        &api_for(&server),
        None,
        &answer_url("1234567890"),
        tmp.path(),
        None,
    ));
    assert_eq!(f.code, ErrorCode::VisitorRequired);
    assert!(f.hint.is_some());
    assert!(server.requests().is_empty());
    assert_eq!(files(tmp.path()), Vec::<String>::new());
    // 不是知乎的链接先被认出来
    let f = failure(fetch::fetch(
        &api_for(&server),
        None,
        "nonsense",
        tmp.path(),
        None,
    ));
    assert_eq!(f.code, ErrorCode::UnsupportedInput);
}

#[test]
fn a_challenge_page_with_a_visitor_identity_means_it_is_no_longer_accepted() {
    let server = Server::start(zhihu);
    // 403 加挑战页（量到的）；200 加挑战页；跳去验证页
    for id in ["2000000001", "2000000002", "2000000013"] {
        let (_tmp, result) = fetch_with(&server, &answer_url(id), None);
        let f = failure(result);
        assert_eq!(f.code, ErrorCode::VisitorRequired, "{id}");
        assert!(
            f.hint.as_deref().is_some_and(|h| h.contains("重新取")),
            "{id}"
        );
        assert_eq!(f.site.as_deref(), Some("www.zhihu.com"));
        assert!(
            f.detail.as_deref().is_some_and(|d| d.contains("zh-zse-ck")),
            "{id} {f:?}"
        );
    }
    // 没有去请求挑战页里的脚本
    assert!(server.requests().iter().all(|r| !r.path.contains("zse")));
}

#[test]
fn deleted_hidden_and_pending_review_are_not_found_with_the_reason() {
    let server = Server::start(zhihu);
    let case = |id: &str| failure(fetch_with(&server, &answer_url(id), None).1);
    let gone = case("2000000003");
    assert_eq!(
        (gone.code, gone.reason),
        (ErrorCode::NotFound, Some(NotFoundReason::Deleted))
    );
    assert!(gone.message.contains("deleted"), "{}", gone.message);
    let review = case("2000000004");
    assert_eq!(
        (review.code, review.reason),
        (ErrorCode::NotFound, Some(NotFoundReason::Private))
    );
    assert!(
        review.message.contains("pending review"),
        "{}",
        review.message
    );
    let hidden = case("2000000005");
    assert_eq!(
        (hidden.code, hidden.reason),
        (ErrorCode::NotFound, Some(NotFoundReason::Private))
    );
    assert!(hidden.message.contains("hidden"), "{}", hidden.message);
    // 数据里没有这一条、页面上也没说为什么
    let unknown = case("2000000006");
    assert_eq!((unknown.code, unknown.reason), (ErrorCode::NotFound, None));
    assert!(unknown.message.contains("answer `2000000006`"));
    // 404、410 的页面
    let f404 = case("2000000009");
    assert_eq!(
        (f404.code, f404.reason),
        (ErrorCode::NotFound, Some(NotFoundReason::Deleted))
    );
    let f410 = case("2000000010");
    assert_eq!(
        (f410.code, f410.reason),
        (ErrorCode::NotFound, Some(NotFoundReason::Deleted))
    );
    assert!(f410.detail.is_some());
    // 没有这个 id 的路径
    assert_eq!(case("9999999999").code, ErrorCode::NotFound);
}

#[test]
fn pages_that_are_not_understood_are_upstream_changed_with_detail() {
    let server = Server::start(zhihu);
    for id in ["2000000007", "2000000008"] {
        let f = failure(fetch_with(&server, &answer_url(id), None).1);
        assert_eq!(f.code, ErrorCode::UpstreamChanged, "{id}");
        assert!(f.detail.is_some(), "{id}");
    }
}

#[test]
fn http_statuses_and_redirects_map_to_the_error_codes() {
    let server = Server::start(zhihu);
    let case = |id: &str| failure(fetch_with(&server, &answer_url(id), None).1);
    let f = case("2000000011");
    assert_eq!((f.code, f.retry_after), (ErrorCode::RateLimited, Some(30)));
    assert_eq!(case("2000000012").code, ErrorCode::Network);
    assert_eq!(case("2000000015").code, ErrorCode::UpstreamChanged);
}

#[test]
fn being_sent_to_the_login_page_with_a_visitor_identity_means_it_is_no_longer_accepted() {
    let server = Server::start(zhihu);
    // 3xx 没被跟（300 不是插件会跟的跳转，原样交回）、`Location` 指向 `/signin`、`/login`，以及 401：都是 `visitor-required`，不是 `login-required`
    for id in ["2000000018", "2000000019", "2000000016"] {
        let f = failure(fetch_with(&server, &answer_url(id), None).1);
        assert_eq!(f.code, ErrorCode::VisitorRequired, "{id}");
        assert!(
            f.hint.as_deref().is_some_and(|h| h.contains("重新取")),
            "{id}"
        );
        assert_eq!(f.site.as_deref(), Some("www.zhihu.com"), "{id}");
    }
    // 跳转的目标留在 message 里
    let f = failure(fetch_with(&server, &answer_url("2000000018"), None).1);
    assert!(f.message.contains("/signin"), "{}", f.message);
    // 同主机的 302 被跟到登录页的 200：一样
    let f = failure(fetch_with(&server, &answer_url("2000000020"), None).1);
    assert_eq!(f.code, ErrorCode::VisitorRequired);
    assert!(f.message.ends_with("/signin"), "{}", f.message);
}

#[test]
fn what_is_not_ours_is_turned_away_without_a_request() {
    let server = Server::start(zhihu);
    for input in [
        "https://www.zhihu.com/zvideo/1234567890123456789",
        "https://www.zhihu.com/collection/78172986",
        "https://example.com/answer/1",
        "nonsense",
    ] {
        let f = failure(fetch_with(&server, input, None).1);
        assert_eq!(f.code, ErrorCode::UnsupportedInput, "{input}");
    }
    assert!(server.requests().is_empty());
}
