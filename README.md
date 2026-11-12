# usuyo-fetch/zhihu

知乎的获取插件，供 [usuyo](https://github.com/usuyo/usuyo) 调用：按链接取回单条回答、文章或想法。

| 方法 | 清单里的 id | 取得 | 账号 | 层级 | 现状 |
|---|---|---|---|---|---|
| 公开页 | `page` | 单条：回答（`www.zhihu.com/question/<问题>/answer/<回答>`、`www.zhihu.com/answer/<回答>`）、文章（`zhuanlan.zhihu.com/p/<id>`）、想法（`www.zhihu.com/pin/<id>`）：正文（Markdown，公式留 LaTeX 源码）、按次序存下的原图、回答的问题标题、作者、创建和编辑时间、计数、页面原样 | 不使用账号；要一份访客身份（在应用里的窗口打开一次知乎，不登录） | `public-web` | 已实现，只对着本机的假服务器测过。2026-11-12 不带 cookie 的普通请求，回答页和文章页都回 403 的挑战页，所以没有访客身份时不发请求、回 `visitor-required`，带着访客身份仍遇到挑战页时也回 `visitor-required`。没有用真实的访客身份请求过，也没有取到过一页真实的页面，页面数据的字段名未验证 |
| 公开收藏夹 | `collection` | 存过的：公开收藏夹里每条的链接、标题、类别、加入时间 | 不使用 | `public-web` | 未实现。2026-11-12 不带 cookie 的普通请求：收藏夹页（200）只有收藏夹自己的信息，没有条目；列条目的 `api/v4/collections/<id>/items` 回 401（「身份未经过验证」）。访客身份够不够不知道 |
| 私密收藏夹 | `collection-private` | 存过的：私密收藏夹 | 登录进 usuyo | `session-api` | 未实现 |

没有移植别的项目的代码；正文转换（`src/markdown.rs`）由同组织的 `usuyo-fetch/wechat-mp` 改来（MIT，同一作者）。
