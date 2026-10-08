## 看运行结果

打开页面链接的四个 RSS 文件。它们使用同一个条目身份和摘要，但有意选择不同的正文：

| 输出 | 正文 |
| --- | --- |
| `summary.xml` | 没有 `content:encoded`，只有 `description` 中的摘要 |
| `portable.xml` | 直接提供的可移植内容，保留 strong 与 URL 链接 |
| `whole.xml` | 整个导出的 body，包括导航和外侧栏 |
| `selected.xml` | article 子树，不包含外面的导航和侧栏 |

## metadata 是本站的输入

`feed-summary` 和 `feed-content` 是这个 Demo 选择的字段。源声明可移植内容，并给渲染的 article 写真实 HTML `id`。之后 API 才把它们作为 `summary` 和 `content` 接收。

{{file:content/index.typ}}

字符串是纯文本，像 HTML 的字符也一样。直接提供的 Typst content 按支持的文本、换行、emphasis、strong、strike 和 URL 链接转换；不会再把它渲染成文档。带样式、延迟内容和不支持的元素会拒绝。正文需要标题、图片或页面格式时，选择导出后的文档 HTML。

## 四个明确的声明

条目的 `id` 用于订阅者识别条目；`(document:, id:)` 中的 `id` 选取恰好一个已经存在的 HTML 元素，两者不是同一概念。选区保留该子树需要的外层结构与样式，并按文档 URL 解析相对资源。

{{file:site/feeds.typ}}

本例通过省略 `content` 生成仅摘要的输出。脚手架的另一份 feed recipe 把缺失或 `none` 的 `feed-content` 默认成整个 body，因此只填摘要不会让那份 recipe 自动变成仅摘要。

## 页面与 feed 一起发布

根程序先输出页面，再声明描述它的 feed。选区 HTML 来自编译后的文档，所有最终输出加入同一次完整构建。固定日期与配置 origin 让结果可重复；将导出的站点用于真实发布前，请改成自己的 origin。

feed URL 使用配置的 `https://example.test`，不是临时预览服务器的地址。本地 preview 用于查看页面与 RSS 文件；feed 内的绝对图片和页面 URL 仍指向这个示例 origin。

{{file:site.typ}}
{{file:tola.toml}}
{{file:site/page.typ}}
