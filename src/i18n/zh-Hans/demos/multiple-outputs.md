## 看运行结果

首页把 **Start** 和 **Write** 列成列表；另一篇文档把相同章节排成表格。`chapters.json` 是真实生成的下载，只保留每章的 `id` 和 `title`。私有输入还带有 `text`，下载有意省略它。

两篇页面都 include `content/index.typ`：源 path 都是 `index.typ`，而 document output 分别是 `index.html` 和 `table/index.html`。`current-source()` 在普通求值时捕获；`current-document()` 则在内容放置位置的 `context` 中读取。

## 同一输入，两套 HTML 组合

根程序读取一次 JSON，把相同值交给两个小函数。它们返回普通 HTML content，不选择文档目的地，也不发布文件。

{{file:static/data/chapters.json}}
{{file:site/chapters.typ}}

## 根程序拥有输出声明

两篇并列 HTML 文档复用页面模板，并 include 同一个源正文。`asset` 把编码后的字节发布到明确 output。读取 `static/data/chapters.json` 不会发布这个输入；把 asset 放在文档旁边，也不同于把内容 include 进页面正文。

{{file:site.typ}}

输出文件名独立于源文件名。链接使用 `output-to-url`，配置的 `/demo/` 恰好添加一次。每个 output 只有一个拥有者。

{{file:content/index.typ}}
{{file:site/page.typ}}
