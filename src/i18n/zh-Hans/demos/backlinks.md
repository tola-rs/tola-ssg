## 看运行结果

打开 **Topic**。Alpha 写了两条指向它的链接，Beta 写了一条原生正文链接；反向链接列表只各显示一次 **/a/** 和 **/b/**。菜单中的链接不计入。首页正文只有一条绝对 URL 指向 Topic，所以首页不出现在列表里，即使该 URL 与 `site.origin` 相同。

## 先选引用，再选页面行

`references(to: auto, from-within: <body>)` 读取写在已标记 article 正文中的原生引用。它返回每次出现的引用，因此 Alpha 贡献两条。后面的 `dedup` 才是本站“每篇链接到这里的文档显示一行”的选择；查询本身保留同一文档中的不同引用。

{{file:site/backlinks.typ}}

反向链接列表放在 `<body>` 之外。它写出的链接不会加入自己读取的区域，从而避免列表不断扩展自身查询。标题的 label 只选中标题；article 的 label 选中包含正文的区域。

## 把导航留在正文区域之外

模板菜单指向全部页面，根程序只给 include 的 article 内容标记 body。同一个 body label 出现在多篇文档时，会选中那些正文区域。Topic 的整篇文档 label 是其他源可链接的原生目标。

{{file:site.typ}}
{{file:content/a.typ}}
{{file:content/b.typ}}
{{file:content/index.typ}}
{{file:content/topic.typ}}

原生目标带有文档 location；URL 目标标识 output。`to: auto` 能接受两种指向当前文档的方式。最终页面、资源和 fragment 是否有效，仍由完整构建检查。
