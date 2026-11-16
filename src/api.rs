//! 知乎的 HTTP 请求：回答、文章、想法的页面，页面里的图片。不读宿主交来的账号凭据，不带任何客户端算出来的参数
//! （`x-zse-96` 一类的签名、`__zse_ck` 一类的挑战 cookie 由插件自己算）。
//! 访客身份（`USUYO_VISITOR`，协议 §2.5.1：用户在应用里的窗口打开一次知乎、不登录，页面自己生成的 cookie）只发给页面的主机
//! `www.zhihu.com`、`zhuanlan.zhihu.com`，只走 https；图片请求不带 cookie。没有访客身份时不发请求，回 `visitor-required`；
//! 带着访客身份仍遇到挑战页、403，或被送去登录页（含 401）时回 `visitor-required`（身份不再被收），保留 `detail`。
//! 请求都经 `usuyo_plugin_kit::http`：页面只连 `www.zhihu.com`、`zhuanlan.zhihu.com`，图片只连 `*.zhimg.com`，只连公网地址。
//!
//! 2026-11-12 实测（这台机器，不带 cookie，桌面浏览器 UA，curl 四次，之后没有再发请求）：
//! - `zhuanlan.zhihu.com/p/19970400`（文章页）：403，650 字节，`<meta id="zh-zse-ck">` 加一个 `static.zhihu.com/zse-ck/…js`，
//!   没有文章数据；响应头 `set-cookie: _xsrf`；
//! - `www.zhihu.com/question/19550225/answer/12345678`（回答页，回答 id 是随手写的，没有证实它存在）：同样的 403 页，650 字节；
//! - `www.zhihu.com/api/v4/collections/78172986/items?offset=0&limit=20`：401，`{"error":{"code":101,"name":"AuthenticationError",
//!   "message":"身份未经过验证"}}`；
//! - `www.zhihu.com/collection/78172986`（公开收藏夹页）：200，53 KB，有 `js-initialData`，但只有收藏夹自己的信息
//!   （标题、条数、公开与否、创建者），没有条目；条目要由页面脚本去调上一条接口。
//!
//! - 经宿主 `usuyo add https://zhuanlan.zhihu.com/p/19970400` 跑了一次（UTC 21:07，临时的 `USUYO_HOME` 和资料库）：本插件的 `page`
//!   请求一次，回挑战页，插件回 `blocked`（`detail` 是挑战页的开头）；30 秒后宿主换内置的通用网页插件请求同一个地址，回 HTTP 403；
//!   条目存成只有链接（`completeness: link`，没有快照），身份是 `zhihu` / `article:19970400`。
//!
//! 所以不带 cookie 的普通请求，回答和文章页只拿到挑战页（要页面脚本算出 `__zse_ck` 才放行，不在「公开页面、不算任何东西」之内）。
//! 方法 `page` 因此改成要一份访客身份（用户在应用的窗口里打开一次知乎，页面脚本自己算出 cookie，宿主存下交来）：没有就不发请求，
//! 回 `visitor-required`；带着访客身份仍遇到挑战页或 403，回 `visitor-required` 并保留 `detail`。不去算、不绕。
//! 没有拿一份真实的访客身份请求过，也没有取到过一页真实的回答、文章或想法：访客身份够不够、`src/page.rs` 对三种页面数据的读法，
//! 都只对着本机的假服务器测过。

use std::time::Duration;

use url::Url;
use usuyo_plugin_kit::http::{self, Client, Cookie, Hosts, Policy};
use usuyo_plugin_kit::protocol::{CallFailure, ErrorCode, NotFoundReason, PluginEvent};

use crate::link::{COLUMN_HOST, HOST};

/// 普通桌面浏览器的 UA（2026-11-12 实测：这个 UA 不带 cookie 取回答、文章页，回挑战页）。
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";
/// 页面上限：收藏夹页实测 53 KB，长回答的页面更大。
const PAGE_BYTES: u64 = 32 << 20;
const ERROR_BODY_BYTES: u64 = 256 << 10;
const PAGE_TIMEOUT: Duration = Duration::from_secs(30);
/// 页面请求最多跟几跳，每一跳都要在 `www.zhihu.com`。
const PAGE_REDIRECTS: u32 = 3;
const IMAGE_TIMEOUT: Duration = Duration::from_secs(30);
/// 文章图片只从这些域名的子域下载，和清单的 `network.hosts` 一致。
const IMAGE_HOST_SUFFIXES: [&str; 1] = [".zhimg.com"];

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("network: {0}")]
    Network(String),
    #[error("HTTP {status}")]
    Http {
        status: u16,
        retry_after: Option<u64>,
        /// 3xx 的 `Location`。
        location: Option<String>,
    },
    /// 跟着跳转落到了登录页（只留路径，不留带着原链接的参数）。
    #[error("the page redirected to the sign-in page {0}")]
    SentToLogin(String),
    #[error("a response is larger than {limit} bytes")]
    TooLarge { limit: u64 },
    #[error("the host `{0}` is not one of the allowed image hosts")]
    ForbiddenHost(String),
    #[error("the address is not an image address: {0}")]
    BadAddress(String),
    /// 连之前的检查没过：主机不在这种请求的名单里，或地址不是公网。
    #[error("refused to connect: {0}")]
    Refused(String),
    /// 平台回了，但不是能用的回答：带着回答的开头，宿主记进失败日志。
    #[error("{error}")]
    Answered {
        error: Box<ApiError>,
        answer: String,
    },
}

impl ApiError {
    /// 图片那一步要停下来报错的：限流。别的错（网络、状态码、太大、主机不在名单上）只记进 `missing`，正文照样交出去。
    pub fn is_hard(&self) -> bool {
        match self {
            ApiError::Http { status, .. } => *status == 429,
            ApiError::Answered { error, .. } => error.is_hard(),
            ApiError::Network(_)
            | ApiError::SentToLogin(_)
            | ApiError::TooLarge { .. }
            | ApiError::ForbiddenHost(_)
            | ApiError::BadAddress(_)
            | ApiError::Refused(_) => false,
        }
    }

    fn answered(self, raw: &[u8]) -> ApiError {
        ApiError::Answered {
            error: Box::new(self),
            answer: String::from_utf8_lossy(raw).into_owned(),
        }
    }
}

impl From<ApiError> for CallFailure {
    fn from(err: ApiError) -> CallFailure {
        failure(&err)
    }
}

fn failure(err: &ApiError) -> CallFailure {
    let message = match err {
        ApiError::Http {
            status,
            location: Some(l),
            ..
        } => format!("HTTP {status} redirecting to {l}"),
        _ => err.to_string(),
    };
    match err {
        ApiError::Network(_) => CallFailure::new(ErrorCode::Network, message),
        ApiError::Http {
            status,
            retry_after,
            location,
        } => match status {
            403 | 412 => blocked(message),
            401 => login_required(message),
            // 跳去登录页的算要登录
            300..=399
                if location.as_deref().is_some_and(|l| {
                    let l = l.to_ascii_lowercase();
                    l.contains("signin") || l.contains("/login")
                }) =>
            {
                login_required(message)
            }
            // 跳去验证页的算被拦；别的跳转是页面改了
            300..=399
                if location.as_deref().is_some_and(|l| {
                    let l = l.to_ascii_lowercase();
                    l.contains("captcha") || l.contains("verify") || l.contains("unhuman")
                }) =>
            {
                blocked(message)
            }
            404 => CallFailure::new(ErrorCode::NotFound, message),
            410 => CallFailure {
                reason: Some(NotFoundReason::Deleted),
                ..CallFailure::new(ErrorCode::NotFound, message)
            },
            429 => CallFailure {
                retry_after: *retry_after,
                hint: Some("知乎提示请求太频繁，请过一会儿再试".into()),
                ..CallFailure::new(ErrorCode::RateLimited, message)
            },
            500..=599 => CallFailure::new(ErrorCode::Network, message),
            _ => CallFailure::new(ErrorCode::UpstreamChanged, message),
        },
        ApiError::SentToLogin(_) => login_required(message),
        ApiError::TooLarge { .. }
        | ApiError::ForbiddenHost(_)
        | ApiError::BadAddress(_)
        | ApiError::Refused(_) => CallFailure::new(ErrorCode::UpstreamChanged, message),
        ApiError::Answered { error, answer } => failure(error).with_detail(answer),
    }
}

pub fn login_required(message: String) -> CallFailure {
    CallFailure {
        hint: Some("知乎要求登录才给这一页".into()),
        ..CallFailure::new(ErrorCode::LoginRequired, message)
    }
}

/// 一次也没有拿到访客身份：不发请求就回。
pub fn visitor_missing() -> CallFailure {
    CallFailure {
        hint: Some(
            "这种方法要一份访客身份：在「获取插件」页的知乎下面取一份（不是你的账号）".into(),
        ),
        site: Some(HOST.into()),
        ..CallFailure::new(
            ErrorCode::VisitorRequired,
            "no visitor identity was handed over (USUYO_VISITOR is not set)".to_owned(),
        )
    }
}

/// 带着访客身份还遇到挑战页、403，或被送去登录页（含 401）：这份身份知乎不再收了。
pub fn visitor_rejected(blocked: CallFailure) -> CallFailure {
    CallFailure {
        code: ErrorCode::VisitorRequired,
        hint: Some(
            "知乎不再收这份访客身份：在「获取插件」页的知乎下面重新取一份（不是你的账号）".into(),
        ),
        site: Some(HOST.into()),
        ..blocked
    }
}

pub fn blocked(message: String) -> CallFailure {
    CallFailure {
        hint: Some(
            "知乎出现了验证页（要页面脚本算出验证 cookie 才放行）；这个插件不去算，可以先在浏览器里打开这条链接再试"
                .into(),
        ),
        ..CallFailure::new(ErrorCode::Blocked, message)
    }
}

/// 要连的主机和地址策略。正式运行用 `production`；测试把它指到本机的假服务器。
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// 测试用：页面请求都发到这个本机地址（路径和查询照旧）。
    local_base: Option<Url>,
    /// 测试用的本机主机：算页面主机，也算允许的图片主机，图片链接不升级成 https。
    local_host: Option<String>,
    policy: Policy,
}

impl Endpoints {
    pub fn production() -> Self {
        Self {
            local_base: None,
            local_host: None,
            policy: Policy::from_host(),
        }
    }

    /// 测试用：`base` 是 `http://127.0.0.1:<端口>`，`policy` 让这个本机地址算公网。
    pub fn local(base: &str, policy: Policy) -> Self {
        let local_base = Url::parse(base).expect("local base URL");
        let local_host = local_base.host_str().map(str::to_owned);
        Self {
            local_base: Some(local_base),
            local_host,
            policy,
        }
    }

    /// 页面请求的主机；访客身份只发给它们，图片请求不带。
    pub fn page_hosts(&self) -> Vec<String> {
        match &self.local_host {
            Some(local) => vec![local.clone()],
            None => vec![HOST.to_owned(), COLUMN_HOST.to_owned()],
        }
    }

    /// 宿主交来的访客身份（`USUYO_VISITOR`），只发给页面的主机、只走 https。没交时是 `None`。
    pub fn visitor_cookie(&self) -> Result<Option<Cookie>, http::VisitorError> {
        Cookie::visitor(self.page_hosts())
    }

    /// 页面请求的地址：正式运行就是链接本身；测试把主机换成本机的假服务器。
    fn page_url(&self, request: &str) -> Result<Url, ApiError> {
        let url = Url::parse(request).map_err(|_| ApiError::BadAddress(request.into()))?;
        let Some(base) = &self.local_base else {
            return Ok(url);
        };
        let mut local = base.clone();
        local.set_path(url.path());
        local.set_query(url.query());
        Ok(local)
    }

    fn is_image_host(&self, host: &str) -> bool {
        self.local_host.as_deref() == Some(host)
            || IMAGE_HOST_SUFFIXES
                .iter()
                .any(|s| host.len() > s.len() && host.ends_with(s))
    }

    /// 页面给的图片地址是不是文章的图片：主机在名单上。名单外的（表情、图标）照样写进 Markdown，不下载、不算缺。
    pub fn is_article_image(&self, raw: &str) -> bool {
        Url::parse(raw)
            .ok()
            .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
            .is_some_and(|h| self.is_image_host(&h))
    }

    fn client(&self, hosts: &[&str]) -> Client {
        let local = self.local_host.as_deref();
        Client::new(USER_AGENT, Hosts::only(hosts.iter().copied().chain(local)))
            .with_policy(self.policy)
    }

    /// 页面给的图片地址：补 scheme、升级成 https、核对主机和端口。
    pub fn image_url(&self, raw: &str) -> Result<Url, ApiError> {
        let raw = raw.trim();
        let full = if raw.starts_with("//") {
            format!("https:{raw}")
        } else {
            raw.to_owned()
        };
        let mut url = Url::parse(&full).map_err(|_| ApiError::BadAddress(raw.to_owned()))?;
        let host = url
            .host_str()
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if !self.is_image_host(&host) {
            return Err(ApiError::ForbiddenHost(host));
        }
        let local = self.local_host.as_deref() == Some(host.as_str());
        // 清单的 `network.hosts` 没写端口，只放行默认端口
        if !local && url.port().is_some() {
            return Err(ApiError::ForbiddenHost(format!(
                "{host}:{}",
                url.port().unwrap_or_default()
            )));
        }
        if url.scheme() == "http" && !local {
            url.set_scheme("https")
                .map_err(|()| ApiError::BadAddress(raw.to_owned()))?;
        }
        if url.scheme() != "https" && !local {
            return Err(ApiError::BadAddress(raw.to_owned()));
        }
        Ok(url)
    }
}

fn transport(e: http::Error) -> ApiError {
    match e {
        http::Error::TooLarge(limit) => ApiError::TooLarge { limit },
        e if e.is_refusal() => ApiError::Refused(e.to_string()),
        e => ApiError::Network(e.to_string()),
    }
}

fn status_error(response: &http::Response) -> ApiError {
    ApiError::Http {
        status: response.status(),
        retry_after: response
            .header("retry-after")
            .and_then(|v| v.trim().parse().ok()),
        location: response.header("location").map(str::to_owned),
    }
}

/// 取回的一页。
pub struct Fetched {
    pub status: u16,
    pub body: Vec<u8>,
}

pub struct Zhihu {
    endpoints: Endpoints,
}

impl Zhihu {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    /// 取页面。`request` 是要请求的链接。状态不是 2xx 就报错，带着回答的开头（读到 256 KB，`with_detail` 再截到 4 KB）。
    pub fn page(&self, request: &str, cookie: &Cookie) -> Result<Fetched, ApiError> {
        let url = self.endpoints.page_url(request)?;
        step("页面");
        let mut response = self
            .endpoints
            .client(&[HOST, COLUMN_HOST])
            .get(&url)
            .header("Accept", "text/html,application/xhtml+xml")
            .header("Accept-Language", "zh-CN,zh;q=0.9")
            .cookie(cookie)
            .follow_redirects(PAGE_REDIRECTS)
            .timeout(PAGE_TIMEOUT)
            .send()
            .map_err(transport)?;
        let status = response.status();
        if !(200..300).contains(&status) {
            let e = status_error(&response);
            let head = response.read_to_vec(ERROR_BODY_BYTES).unwrap_or_default();
            return Err(e.answered(&head));
        }
        // 同一个主机上的跳转已经跟了：落在登录页时回的是 200，照被送去登录页报
        let landed = response.url().path().to_ascii_lowercase();
        if landed != url.path().to_ascii_lowercase()
            && (landed.starts_with("/signin") || landed.starts_with("/login"))
        {
            return Err(ApiError::SentToLogin(response.url().path().to_owned()));
        }
        let body = response.read_to_vec(PAGE_BYTES).map_err(transport)?;
        Ok(Fetched { status, body })
    }

    /// 下载一张图到内存，最多 `limit` 字节。返回内容和响应的 `Content-Type`。不带 cookie、不带 Referer。
    pub fn image(&self, url: &Url, limit: u64) -> Result<(Vec<u8>, Option<String>), ApiError> {
        let mut response = self
            .endpoints
            .client(&IMAGE_HOST_SUFFIXES)
            .get(url)
            .header("Accept", "image/*")
            .follow_redirects(2)
            .timeout(IMAGE_TIMEOUT)
            .send()
            .map_err(transport)?;
        if !(200..300).contains(&response.status()) {
            return Err(status_error(&response));
        }
        let content_type = response.header("content-type").map(str::to_owned);
        let body = response.read_to_vec(limit).map_err(transport)?;
        Ok((body, content_type))
    }
}

/// 开始一步之前告诉宿主：一步一步慢下来时，宿主照样收得到 progress，不按「没进度」杀掉进程。
pub(crate) fn step(label: &str) {
    let event = PluginEvent::Progress {
        done: None,
        total: None,
        unit: None,
        message: Some(label.into()),
    };
    eprintln!("{}", event.stderr_line());
}

pub(crate) fn progress(done: usize, total: usize, label: &str) {
    let event = PluginEvent::Progress {
        done: Some(done as f64),
        total: Some(total as f64),
        unit: Some("张".into()),
        message: Some(label.into()),
    };
    eprintln!("{}", event.stderr_line());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_hosts() {
        let e = Endpoints::production();
        let ok = |s: &str| e.image_url(s).map(|u| u.to_string());
        assert_eq!(
            ok("http://pic1.zhimg.com/v2-aaaa_r.jpg").unwrap(),
            "https://pic1.zhimg.com/v2-aaaa_r.jpg"
        );
        assert!(ok("https://picx.zhimg.com/v2-bbbb_r.png?source=1").is_ok());
        assert!(ok("//pic2.zhimg.com/v2-cccc_r.jpg").is_ok());
        assert!(matches!(
            ok("https://pic1.zhimg.com:8443/a"),
            Err(ApiError::ForbiddenHost(_))
        ));
        for bad in [
            "https://zhimg.com/a",
            "https://evilzhimg.com/a",
            "https://pic1.zhimg.com.evil.test/a",
            "https://static.zhihu.com/a.png",
            "https://127.0.0.1/a",
        ] {
            assert!(matches!(ok(bad), Err(ApiError::ForbiddenHost(_))), "{bad}");
        }
        assert!(matches!(
            ok("ftp://pic1.zhimg.com/a"),
            Err(ApiError::BadAddress(_))
        ));
        assert!(matches!(ok("nonsense"), Err(ApiError::BadAddress(_))));
        assert!(e.is_article_image("https://pic1.zhimg.com/a"));
        assert!(!e.is_article_image("https://static.zhihu.com/a.png"));
    }

    #[test]
    fn failure_mapping() {
        let http = |status, location: Option<&str>| {
            CallFailure::from(ApiError::Http {
                status,
                retry_after: Some(7),
                location: location.map(str::to_owned),
            })
        };
        assert_eq!(http(403, None).code, ErrorCode::Blocked);
        assert_eq!(http(401, None).code, ErrorCode::LoginRequired);
        assert_eq!(
            http(302, Some("https://www.zhihu.com/signin?next=%2F")).code,
            ErrorCode::LoginRequired
        );
        assert_eq!(http(412, None).code, ErrorCode::Blocked);
        assert_eq!(http(429, None).retry_after, Some(7));
        assert_eq!(http(502, None).code, ErrorCode::Network);
        assert_eq!(http(404, None).code, ErrorCode::NotFound);
        assert_eq!(http(410, None).reason, Some(NotFoundReason::Deleted));
        assert_eq!(
            http(302, Some("https://example.com/x")).code,
            ErrorCode::UpstreamChanged
        );
        assert_eq!(
            http(302, Some("https://www.zhihu.com/account/unhuman?type=Q")).code,
            ErrorCode::Blocked
        );
    }
}
