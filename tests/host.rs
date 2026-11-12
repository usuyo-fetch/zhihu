//! 真的二进制：不需要网络的子命令，以及清单经宿主的校验和调用。

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;
use usuyo_plugin::protocol::{Access, Account, Canonicalized, ErrorCode, Identity, Login};
use usuyo_plugin::{
    CallLimits, CallOptions, FetchInput, FetchRequest, HostConfig, PluginError, PluginHost,
    discover,
};

const BIN: &str = env!("CARGO_BIN_EXE_usuyo-fetch-zhihu");

/// 清空环境后运行，和宿主一样。
fn run(args: &[&str]) -> (i32, Value) {
    let output = Command::new(BIN).args(args).env_clear().output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json = if stdout.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&stdout).unwrap()
    };
    (output.status.code().unwrap(), json)
}

#[test]
fn canonicalize_needs_no_network() {
    for (input, content_id, url) in [
        (
            "https://www.zhihu.com/question/19550225/answer/1234567890?utm_source=wechat",
            "answer:1234567890",
            "https://www.zhihu.com/answer/1234567890",
        ),
        (
            "https://zhuanlan.zhihu.com/p/19970400",
            "article:19970400",
            "https://zhuanlan.zhihu.com/p/19970400",
        ),
        (
            "https://www.zhihu.com/pin/1500000000000000001",
            "pin:1500000000000000001",
            "https://www.zhihu.com/pin/1500000000000000001",
        ),
    ] {
        let (code, out) = run(&["canonicalize", input]);
        assert_eq!(code, 0, "{input}");
        assert_eq!(out["match"], true);
        assert_eq!(out["identity"]["platform"], "zhihu");
        assert_eq!(out["identity"]["contentId"], content_id);
        assert_eq!(out["identity"]["url"], url);
        assert!(out.get("resolvedUrl").is_none());
    }
    for input in [
        "https://www.zhihu.com/zvideo/1234567890123456789",
        "https://www.zhihu.com/collection/78172986",
        "https://example.com/answer/1",
    ] {
        let (code, out) = run(&["canonicalize", input]);
        assert_eq!((code, &out["match"]), (0, &Value::Bool(false)), "{input}");
    }
}

#[test]
fn health_does_not_touch_the_network() {
    for args in [&["health"][..], &["health", "--probe"][..]] {
        let (code, out) = run(args);
        assert_eq!(code, 0);
        assert_eq!(
            (out["ready"].as_bool(), out["protocol"].as_u64()),
            (Some(true), Some(1))
        );
        assert_eq!(out["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(out["login"], serde_json::json!([]));
        assert_eq!(out["checks"], serde_json::json!([]));
    }
}

#[test]
fn bad_usage_exits_2() {
    for args in [
        &[][..],
        &["nope"][..],
        &["fetch", "x"][..],
        &["fetch", "x", "--out", "/o"][..],
        &["sources"][..],
        &["canonicalize"][..],
    ] {
        let (code, out) = run(args);
        assert_eq!((code, out), (2, Value::Null), "{args:?}");
    }
}

#[test]
fn each_call_goes_to_its_method() {
    // 方法和调用对不上的、没做的方法，回 invalid-args，不发请求
    let link = "https://www.zhihu.com/answer/1234567890";
    for args in [
        &["sources", "--method", "page"][..],
        &[
            "sources",
            "--method",
            "collection",
            "--link",
            "https://www.zhihu.com/collection/1",
        ][..],
        &["list", "zhihu:collection:1", "--method", "collection"][..],
        &["list", "zhihu:favorites", "--method", "collection-private"][..],
        &["fetch", link, "--out", "/o", "--method", "collection"][..],
        &["fetch", link, "--out", "/o", "--method", "nope"][..],
    ] {
        let (code, out) = run(args);
        assert_eq!(
            (code, &out["error"]["code"]),
            (1, &Value::from("invalid-args")),
            "{args:?}"
        );
    }
    // 不是知乎回答、文章、想法的链接：不发请求
    let tmp = tempfile::tempdir().unwrap();
    let (code, out) = run(&[
        "fetch",
        "https://www.zhihu.com/zvideo/1234567890123456789",
        "--out",
        tmp.path().to_str().unwrap(),
        "--method",
        "page",
    ]);
    assert_eq!(code, 1);
    assert_eq!(out["error"]["code"], "unsupported-input");
}

#[test]
fn page_without_a_visitor_identity_answers_visitor_required_before_any_request() {
    let tmp = tempfile::tempdir().unwrap();
    // 环境清空：没有 USUYO_VISITOR；真的发了请求会是 network 或别的错
    let (code, out) = run(&[
        "fetch",
        "https://www.zhihu.com/answer/1234567890",
        "--out",
        tmp.path().to_str().unwrap(),
        "--method",
        "page",
    ]);
    assert_eq!(
        (code, &out["error"]["code"]),
        (1, &Value::from("visitor-required"))
    );
    assert_eq!(out["error"]["site"], "www.zhihu.com");
}

/// 清单装成宿主认的样子：`<root>/usuyo-fetch/zhihu/<version>/`。
fn host() -> (tempfile::TempDir, PluginHost) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("plugins/usuyo-fetch/zhihu/0.1.0");
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("usuyo-plugin.yaml");
    std::fs::copy(manifest, dir.join("usuyo-plugin.yaml")).unwrap();
    std::fs::copy(BIN, dir.join("usuyo-fetch-zhihu")).unwrap();
    let found = discover(&tmp.path().join("plugins")).unwrap();
    assert!(found.rejected.is_empty(), "{:?}", found.rejected);
    let host = PluginHost::new(
        HostConfig {
            work_dir: tmp.path().join("work"),
            cache_dir: tmp.path().join("cache"),
            locale: "zh-CN".into(),
            limits: CallLimits::default(),
        },
        found.fetch,
    );
    (tmp, host)
}

#[tokio::test]
async fn the_manifest_lists_every_row_of_routes_and_matches_the_three_kinds() {
    let (_tmp, host) = host();
    let plugin = &host.plugins()[0];
    assert_eq!(plugin.id().as_str(), "usuyo-fetch/zhihu");
    assert_eq!(plugin.header.version.to_string(), env!("CARGO_PKG_VERSION"));
    let spec = &plugin.spec;
    // routes.md 第 10 节的每一行，顺序照清单
    let ids: Vec<&str> = spec.methods.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["page", "collection", "collection-private"]);
    let page = spec.method("page").unwrap();
    assert_eq!(
        (page.access, page.login, page.account),
        (Access::PublicWeb, Login::None, Account::Visitor)
    );
    assert_eq!(page.visitors.len(), 1);
    assert_eq!(page.visitors[0].as_str(), "www.zhihu.com");
    assert!(page.built && page.sites.is_empty() && page.capabilities.fetch.is_some());
    let collection = spec.method("collection").unwrap();
    assert!(!collection.built);
    assert_eq!(
        (collection.access, collection.login, collection.account),
        (Access::PublicWeb, Login::None, Account::None)
    );
    // 列表能力的链接框：导入页据此给一个链接框
    assert!(
        collection
            .capabilities
            .list
            .as_ref()
            .unwrap()
            .link
            .is_some()
    );
    let private = spec.method("collection-private").unwrap();
    assert!(!private.built);
    assert_eq!(
        (private.access, private.login, private.account),
        (Access::SessionApi, Login::Required, Account::InUsuyo)
    );

    for hit in [
        "https://www.zhihu.com/question/19550225/answer/1234567890",
        "https://www.zhihu.com/answer/1234567890",
        "https://zhuanlan.zhihu.com/p/19970400",
        "https://www.zhihu.com/pin/1500000000000000001",
    ] {
        let url = url::Url::parse(hit).unwrap();
        assert_eq!(host.candidates(&url).len(), 1, "{hit}");
    }
    for miss in [
        "https://www.zhihu.com/zvideo/1234567890123456789",
        "https://www.zhihu.com/collection/78172986",
        "https://www.zhihu.com/people/someone",
        "https://example.com/answer/1",
    ] {
        let url = url::Url::parse(miss).unwrap();
        // zvideo 的页面归 yt-dlp 插件；这里的 match 里没有它
        assert!(host.candidates(&url).is_empty(), "{miss}");
    }

    // 经宿主调一次：输出要过宿主的 schema 校验
    let url = url::Url::parse("https://www.zhihu.com/question/19550225/answer/1234567890").unwrap();
    let canonical = host
        .canonicalize(host.candidates(&url)[0], &url, &CallOptions::default())
        .await
        .unwrap();
    let Canonicalized::Match {
        identity: Identity::Platform { content_id, .. },
        ..
    } = canonical
    else {
        panic!("{canonical:?}")
    };
    assert_eq!(content_id, "answer:1234567890");
}

#[tokio::test]
async fn a_link_that_is_not_ours_is_turned_away_through_the_host() {
    let (_tmp, host) = host();
    let plugin = &host.plugins()[0];
    let url = url::Url::parse("https://www.zhihu.com/answer/abc").unwrap();
    let request = FetchRequest {
        method: "page".into(),
        input: FetchInput::Url(url),
        want: None,
        input_file: None,
    };
    let err = host
        .fetch(plugin, request, &CallOptions::default())
        .await
        .unwrap_err();
    let PluginError::Call { failure, .. } = err else {
        panic!("{err:?}")
    };
    assert_eq!(failure.code, ErrorCode::UnsupportedInput);
}
