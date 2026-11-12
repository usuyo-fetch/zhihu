//! 认链接：一条知乎链接是哪一条回答、文章或想法。不发请求。
//!
//! 认的形状（主机 `www.zhihu.com`、`zhihu.com`、`zhuanlan.zhihu.com`，http 或 https）：
//! - `/question/<问题 id>/answer/<回答 id>` 和 `/answer/<回答 id>`：回答；
//! - `zhuanlan.zhihu.com/p/<id>`：文章；
//! - `/pin/<id>`：想法。
//!
//! 知乎视频 `/zvideo/<id>` 归 yt-dlp 插件，这里不认。
//!
//! 身份：`answer:<id>`、`article:<id>`、`pin:<id>`，写法和 yt-dlp 插件的 `zvideo:<id>` 一致。
//! 同一条回答的两种链接身份相同；规范链接统一写 `https://www.zhihu.com/answer/<id>`。

use url::Url;
use usuyo_plugin_kit::protocol::{CallFailure, Canonicalized, ErrorCode, Identity, PlatformId};

pub const PLATFORM: &str = "zhihu";
pub const HOST: &str = "www.zhihu.com";
pub const COLUMN_HOST: &str = "zhuanlan.zhihu.com";

/// 粘贴的文字最多看这么多字节。
const MAX_TEXT: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Answer,
    Article,
    Pin,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Answer => "answer",
            Kind::Article => "article",
            Kind::Pin => "pin",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub kind: Kind,
    pub id: String,
    /// 回答的链接里带着问题 id 时。
    pub question: Option<String>,
}

impl Target {
    pub fn content_id(&self) -> String {
        format!("{}:{}", self.kind.name(), self.id)
    }

    pub fn canonical_url(&self) -> String {
        match self.kind {
            Kind::Answer => format!("https://{HOST}/answer/{}", self.id),
            Kind::Article => format!("https://{COLUMN_HOST}/p/{}", self.id),
            Kind::Pin => format!("https://{HOST}/pin/{}", self.id),
        }
    }

    /// 取页面时请求的链接：回答带着问题 id 时用问题下的写法（不用多跳一次重定向）。
    pub fn request_url(&self) -> String {
        match (self.kind, &self.question) {
            (Kind::Answer, Some(q)) => format!("https://{HOST}/question/{q}/answer/{}", self.id),
            _ => self.canonical_url(),
        }
    }

    pub fn identity(&self) -> Result<Identity, CallFailure> {
        let platform =
            PlatformId::try_from(PLATFORM).map_err(|e| CallFailure::new(ErrorCode::Internal, e))?;
        Ok(Identity::Platform {
            platform,
            content_id: self.content_id(),
            url: Some(self.canonical_url()),
        })
    }
}

/// 输入是一条链接，或带链接的分享文字。`None` 是「不是知乎回答、文章、想法的链接」，不是错误。
pub fn parse_input(input: &str) -> Option<Target> {
    let url = find_url(input)?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let segments: Vec<&str> = url.path().trim_matches('/').split('/').collect();
    let target = |kind, id: &&str, question: Option<&&str>| {
        is_id(id).then(|| Target {
            kind,
            id: (*id).to_owned(),
            question: question.filter(|q| is_id(q)).map(|q| (*q).to_owned()),
        })
    };
    match (host.as_str(), segments.as_slice()) {
        ("www.zhihu.com" | "zhihu.com", ["question", q, "answer", id]) if is_id(q) => {
            target(Kind::Answer, id, Some(q))
        }
        ("www.zhihu.com" | "zhihu.com", ["answer", id]) => target(Kind::Answer, id, None),
        ("www.zhihu.com" | "zhihu.com", ["pin", id]) => target(Kind::Pin, id, None),
        ("zhuanlan.zhihu.com", ["p", id]) => target(Kind::Article, id, None),
        _ => None,
    }
}

fn is_id(s: &str) -> bool {
    (1..=20).contains(&s.len()) && s.chars().all(|c| c.is_ascii_digit())
}

/// 整个输入是一条链接就用它；不然取文字里第一个 `http(s)://` 起到空白或非 ASCII 字符为止的一段。
fn find_url(input: &str) -> Option<Url> {
    let end = (0..=MAX_TEXT.min(input.len()))
        .rev()
        .find(|&i| input.is_char_boundary(i))?;
    let text = input[..end].trim().replace("&amp;", "&");
    if let Ok(url) = Url::parse(&text)
        && matches!(url.scheme(), "http" | "https")
    {
        return Some(url);
    }
    let start = ["https://", "http://"]
        .iter()
        .filter_map(|p| text.find(p))
        .min()?;
    let rest = &text[start..];
    let stop = rest
        .find(|c: char| !c.is_ascii_graphic() || matches!(c, '"' | '\'' | '<' | '>'))
        .unwrap_or(rest.len());
    let candidate = rest[..stop].trim_end_matches(['.', ',', ';', ')', ']', '!']);
    Url::parse(candidate).ok()
}

pub fn canonicalize(input: &str) -> Result<Canonicalized, CallFailure> {
    Ok(match parse_input(input) {
        Some(target) => Canonicalized::Match {
            identity: target.identity()?,
            resolved_url: None,
        },
        None => Canonicalized::NoMatch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_in_both_forms_share_one_identity() {
        for input in [
            "https://www.zhihu.com/question/19550225/answer/1234567890?utm_source=wechat",
            "http://zhihu.com/answer/1234567890/",
            "https://www.zhihu.com/answer/1234567890#comments",
            "回答 https://www.zhihu.com/question/19550225/answer/1234567890，复制链接",
        ] {
            let t = parse_input(input).unwrap_or_else(|| panic!("{input}"));
            assert_eq!(t.content_id(), "answer:1234567890", "{input}");
            assert_eq!(t.canonical_url(), "https://www.zhihu.com/answer/1234567890");
        }
        let with_question =
            parse_input("https://www.zhihu.com/question/19550225/answer/1234567890").unwrap();
        assert_eq!(
            with_question.request_url(),
            "https://www.zhihu.com/question/19550225/answer/1234567890"
        );
    }

    #[test]
    fn articles_and_pins() {
        let a = parse_input("https://zhuanlan.zhihu.com/p/19970400?utm=x").unwrap();
        assert_eq!(a.content_id(), "article:19970400");
        assert_eq!(a.canonical_url(), "https://zhuanlan.zhihu.com/p/19970400");
        let p = parse_input("https://www.zhihu.com/pin/1500000000000000001").unwrap();
        assert_eq!(p.content_id(), "pin:1500000000000000001");
        assert_eq!(
            p.canonical_url(),
            "https://www.zhihu.com/pin/1500000000000000001"
        );
    }

    #[test]
    fn what_is_not_ours_is_turned_away() {
        for input in [
            "https://www.zhihu.com/zvideo/1234567890123456789",
            "https://www.zhihu.com/question/19550225",
            "https://www.zhihu.com/question/abc/answer/1",
            "https://www.zhihu.com/collection/78172986",
            "https://www.zhihu.com/people/someone",
            "https://zhuanlan.zhihu.com/p/",
            "https://zhuanlan.zhihu.com/c_1234",
            "https://www.zhihu.com.evil.test/answer/1",
            "https://example.com/answer/1",
            "ftp://www.zhihu.com/answer/1",
            "https://www.zhihu.com/answer/1/extra",
            "nonsense",
            "",
        ] {
            assert_eq!(parse_input(input), None, "{input}");
        }
    }

    #[test]
    fn canonicalize_envelopes() {
        assert!(matches!(
            canonicalize("https://zhuanlan.zhihu.com/p/1").unwrap(),
            Canonicalized::Match { .. }
        ));
        assert!(matches!(
            canonicalize("https://example.com/").unwrap(),
            Canonicalized::NoMatch
        ));
    }
}
