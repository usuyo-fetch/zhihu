# usuyo-fetch/zhihu

知乎的获取插件，供 [usuyo](https://github.com/usuyo/usuyo) 调用：按链接取回单条回答、文章或想法。

| 方法 | 清单里的 id | 取得 | 账号 | 层级 | 现状 |
|---|---|---|---|---|---|
| 公开页 | `page` | 单条：回答（`www.zhihu.com/question/<问题>/answer/<回答>`、`www.zhihu.com/answer/<回答>`）、文章（`zhuanlan.zhihu.com/p/<id>`）、想法（`www.zhihu.com/pin/<id>`）：正文（Markdown，公式留 LaTeX 源码）、按次序存下的原图、回答的问题标题、作者、创建和编辑时间、计数、页面原样 | 不使用账号；要一份访客身份（在应用里的窗口打开一次知乎，不登录） | `public-web` | 已实现，只对着本机的假服务器测过。2026-11-12 不带 cookie 的普通请求，回答页和文章页都回 403 的挑战页，所以没有访客身份时不发请求、回 `visitor-required`，带着访客身份仍遇到挑战页时也回 `visitor-required`。没有用真实的访客身份请求过，也没有取到过一页真实的页面，页面数据的字段名未验证 |
| 公开收藏夹 | `collection` | 存过的：公开收藏夹里每条的链接、标题、类别、加入时间 | 不使用 | `public-web` | 未实现。2026-11-12 不带 cookie 的普通请求：收藏夹页（200）只有收藏夹自己的信息，没有条目；列条目的 `api/v4/collections/<id>/items` 回 401（「身份未经过验证」）。访客身份够不够不知道 |
| 私密收藏夹 | `collection-private` | 存过的：私密收藏夹 | 登录进 usuyo | `session-api` | 未实现 |
| MediaCrawler 的结果文件 | `mediacrawler` | 存下的：你自己跑 MediaCrawler 留下的知乎帖子文件（json 或 jsonl，或把 data 文件夹压成的 zip）里每篇的链接、标题、作者昵称、发布时间；不读评论、CSV、SQLite；只列链接，内容交给本插件的其他方法去取 | 不使用（文件是使用者自己用登录了账号的 MediaCrawler 跑出来的，这种方法不联网） | `private-api`，默认关 | 已实现，只对着手写的文件测过，没有对着 MediaCrawler 真正的输出比对过 |

没有移植别的项目的代码；正文转换（`src/markdown.rs`）由同组织的 `usuyo-fetch/wechat-mp` 改来（MIT，同一作者）。

## 开发

`Cargo.toml` 里的 `usuyo-plugin`、`usuyo-plugin-kit` 是对 `https://github.com/usuyo/usuyo` 的 git 依赖，钉在固定的 `rev`。

本地开发时用旁边 `usuyo/` 的工作区代替它们：`usuyo-fetch/.cargo/config.toml`（不进任何插件仓库）写了下面的覆盖，cargo 从当前目录往上找这个文件，在插件文件夹里跑 cargo 就生效；从别处跑要加 `--config <这个文件>`。

```toml
[patch."https://github.com/usuyo/usuyo"]
usuyo-plugin = { path = "../usuyo/crates/usuyo-plugin" }
usuyo-plugin-kit = { path = "../usuyo/crates/usuyo-plugin-kit" }
```

`Cargo.lock` 记的是覆盖后的路径形式，所以 CI 不加 `--locked` 构建，cargo 只重新锁这两个 usuyo crate。
