## 看运行结果

首页目录包含 **Start** 和 **Detail**。`Deep detail` 超过指定深度，`Aside` 设置了 `outlined: false`。另一篇文档的目录只有 **Outside the guide**。逐项点击，链接会落在对应文档的对应标题上。

## 查询调用所在的文档

这个函数返回 contextual content，在内容放置的位置执行，因此 `current-document()` 和 `headings()` 属于那篇文档。源文件不是页面身份：同一个源也可以被其他文档 include。

{{file:site/toc.typ}}

depth 按声明的标题 level 过滤；是否 outlined 是另一项选择。链接原生 location 可以同时处理带 label 和不带 label 的标题，不必编造或保存 `loc-N` 名称。

## 每篇文档放自己的目录

根程序按 path 选源，不依赖数组下标，并把同一个目录函数放进两篇文档。

{{file:site.typ}}

## label 与 outline 可见性

`<start>` 为 Start 提供稳定 label。Detail 没写 label，但目录链接使它生成 anchor。链接落在哪里，由最终输出确定，而不是由某个生成名称的数字决定。

{{file:content/index.typ}}
{{file:content/other.typ}}
{{file:site/page.typ}}
