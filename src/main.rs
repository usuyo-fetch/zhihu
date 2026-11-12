//! `usuyo-fetch-zhihu`：子命令照协议 §3.2，stdout 只写一个信封。

use std::process::ExitCode;

use anyhow::Context;
use serde::Serialize;
use usuyo_fetch_zhihu::api::{Endpoints, Zhihu};
use usuyo_fetch_zhihu::{fetch, link};
use usuyo_plugin_kit::fetch::{Call, USAGE};
use usuyo_plugin_kit::protocol::{CallFailure, Envelope, ErrorCode, FetchHealth};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::from(1)
        }
    }
}

/// 分派到清单里的方法：只有 `page` 在插件进程里跑，收藏夹两种还没做。
fn run(args: &[String]) -> anyhow::Result<u8> {
    let Some(call) = Call::parse(args) else {
        eprintln!("{USAGE}");
        return Ok(2);
    };
    let method = call.method().map(str::to_owned);
    match (method.as_deref(), call) {
        (_, Call::Canonicalize { url }) => emit(link::canonicalize(&url)),
        (
            Some("page"),
            Call::Fetch {
                input, out, want, ..
            },
        ) => {
            let api = Zhihu::new(Endpoints::production());
            // 访客身份文件读不了、形状不对：宿主写的，不是用户填的，报 internal，不带值
            let cookie = api
                .endpoints()
                .visitor_cookie()
                .map_err(|e| CallFailure::new(ErrorCode::Internal, e.to_string()));
            emit(cookie.and_then(|cookie| {
                fetch::fetch(&api, cookie.as_ref(), &input, &out, want.as_deref())
            }))
        }
        // 没有要查的本机程序，也不向平台发请求
        (_, Call::Health { .. }) => emit(Ok(FetchHealth::new(env!("CARGO_PKG_VERSION")))),
        (method, _) => emit::<()>(Err(CallFailure::new(
            ErrorCode::InvalidArgs,
            format!("this call does not apply to method {method:?}"),
        ))),
    }
}

/// 把结果写成 stdout 上的那一个信封，返回退出码。
fn emit<T: Serialize>(result: Result<T, CallFailure>) -> anyhow::Result<u8> {
    let envelope = match result {
        Ok(body) => Envelope::Ok(body),
        Err(failure) => Envelope::Err(failure),
    };
    println!(
        "{}",
        serde_json::to_string(&envelope).context("cannot write the envelope")?
    );
    Ok(u8::from(envelope.exit_code() != 0))
}
