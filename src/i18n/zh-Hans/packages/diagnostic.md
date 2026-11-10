# `@tola/diagnostic`

从 Typst 运行中的代码提交结构化报告

## `report`

在不中断 Typst 求值的情况下报告问题。

默认位置是本函数的实际调用处。把 content 传给 `at` 会改用该内容自身的原生源码位置；传 `none` 则得到一个没有源码位置的问题。生成或经过变换的内容可能没有位置，此时不会猜测源码位置。排版位置、字符串路径或字典都不能代替源码内容。

Tola 收集 error、warning、info 和 hint 四种级别的报告。error 会让求值之后的检查或发布失败；要立即停止求值，请改用 Typst 自身的错误路径。报告和它可选的规则代码都归调用的项目或包所有。

Example - 针对作者写的内容报告问题，附上提示，并关联另一个源码值：

```typst
#import "@tola/diagnostic:0.0.0": report
#let value = [Sample]
#let original = [Earlier sample]
#report("Review this value.", severity: "warning", code: "review-value", at: value,
  hints: ("Choose a value supported by this module.",),
  related: ((message: "Earlier value.", at: original),))
```

### `message`

问题的说明。

### `severity`

`"error"`、`"warning"`、`"info"` 或 `"hint"`，默认为 `"warning"`。

### `code`

可选的、由调用方负责的非空规则代码。

### `at`

调用处本身、某个内容值的原生位置，或没有位置。

### `hints`

用作解释的字符串数组。

### `related`

字典数组，每项只有 `message`（str）和 `at`（content 或 none）。
