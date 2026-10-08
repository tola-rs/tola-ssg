## 看运行结果

导航依次是 **Sources become pages → Advanced → Getting started**。`Advanced` 发布在 `/demo/chosen/`；草稿不发布。声明 metadata 的 JSON 仍保留标题两端的空格和草稿：解析改变返回的记录，不改源声明。

## 先声明、校验，再选择

`title`、`draft`、`order` 和 `permalink` 是这个站点的约定。全部源先解析，再过滤 draft。草稿中的 Typst 或 schema 错误仍会使构建失败。这份 schema 拒绝未知字段；需要新字段时先在这里声明。

{{file:site/schema.typ}}

`optional` 处理缺失的键；`nullable` 接受显式 `none`。标题先 trim，再由 `non-empty` 检查。编辑顺序显式排序，源身份用来打破相同 order 的并列。permalink 恰好解码一次，然后转换成 output。

{{file:site/selection.typ}}

## 复用选择结果

根程序取得完整源记录，生成 `(source:, output:)`，再交给导航和页面模板。`source.file` 是 Typst 输入路径，不是浏览器 URL。目的地只算一次，导航和文档就能使用同一条路由。

{{file:site.typ}}
{{file:site/navigation.typ}}

## 声明标题与页面标题

声明保留空格；schema 返回的值为文档和导航提供整理后的标题。`#title()` 读取根程序赋给文档的标题。

{{file:content/start.typ}}
{{file:content/advanced.typ}}
{{file:content/draft.typ}}

共享模板提供浏览器 head 与样式表。配置决定部署路径；output 不带部署前缀。

{{file:site/page.typ}}
{{file:tola.toml}}
