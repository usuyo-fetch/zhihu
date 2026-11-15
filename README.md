# usuyo-fetch/zhihu

知乎的获取插件，供 [usuyo](https://github.com/usuyo/usuyo) 调用：按链接取回单条回答、文章或想法。

| 功能 | 实现方式（方法 id） | 账号 | 风险层级 | 状态 |
|---|---|---|---|---|
| 按链接取单条内容（回答、文章、想法） | 公开页（`page`） | 访客身份（在应用里的窗口打开一次知乎，不登录），不使用账号 | `public-web`，默认开 | 已实现（只对着本机的假服务器测过，没有取到过真实页面） |
| 视频文件 | — | — | — | 未实现 |
| 列出我的收藏 | 公开收藏夹（`collection`） | 不使用 | `public-web`，默认开 | 未实现 |
| 列出我的收藏 | 私密收藏夹（`collection-private`） | 登录进 usuyo | `session-api`，需要登录，登录后开 | 未实现 |

## 方法

### `page`：公开页

取单条：回答（`www.zhihu.com/question/<问题>/answer/<回答>`、`www.zhihu.com/answer/<回答>`）、文章（`zhuanlan.zhihu.com/p/<id>`）、想法（`www.zhihu.com/pin/<id>`）。取得正文（Markdown，公式留 LaTeX 源码）、按次序存下的原图、回答的问题标题、作者、创建和编辑时间、计数、页面原样。页面里的视频不下载，只把地址写进正文的链接和 `meta.json`。2026-11-12 不带 cookie 的普通请求，回答页和文章页都回 403 的挑战页，所以没有访客身份时不发请求、回 `visitor-required`；带着访客身份仍遇到挑战页时也回 `visitor-required`。没有用真实的访客身份请求过，也没有取到过一页真实的页面，页面数据的字段名未验证。节奏：同一平台批量时间隔 30 秒、每天 100 条。

### `collection`：公开收藏夹

未实现。2026-11-12 不带 cookie 的普通请求：收藏夹页（200）只有收藏夹自己的信息，没有条目；列条目的 `api/v4/collections/<id>/items` 回 401（「身份未经过验证」）。访客身份够不够不知道。

### `collection-private`：私密收藏夹

未实现。清单里列出，登录进 usuyo 后用登录 cookie 取；调研里没有处罚记录。

没有移植别的项目的代码；正文转换（`src/markdown.rs`）由同组织的 `usuyo-fetch/wechat` 改来（MIT，同一作者）。

## 开发

`Cargo.toml` 里的 `usuyo-plugin`、`usuyo-plugin-kit` 是对 `https://github.com/usuyo/usuyo` 的 git 依赖，钉在固定的 `rev`。

本地开发时用旁边 `usuyo/` 的工作区代替它们：`usuyo-fetch/.cargo/config.toml`（不进任何插件仓库）写了下面的覆盖，cargo 从当前目录往上找这个文件，在插件文件夹里跑 cargo 就生效；从别处跑要加 `--config <这个文件>`。

```toml
[patch."https://github.com/usuyo/usuyo"]
usuyo-plugin = { path = "../usuyo/crates/usuyo-plugin" }
usuyo-plugin-kit = { path = "../usuyo/crates/usuyo-plugin-kit" }
```

`Cargo.lock` 记的是覆盖后的路径形式，所以 CI 不加 `--locked` 构建，cargo 只重新锁这两个 usuyo crate。
