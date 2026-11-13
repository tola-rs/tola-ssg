# `@tola/diagnostic`

在 Typst 中报告警告和错误。

## `report`

报告问题后继续处理 Typst 文档。

默认在调用处显示消息。要指出某个标题或链接，把该元素传给 `at`。传 `none` 时不显示位置。

检查或构建遇到 `error` 会失败，遇到 `warning` 可以继续。想直接停下 Typst 编译，用 `panic`。


把下面的代码放到 `tola init` 站点的 `content/index.typ`。运行 `tola check`，警告会指向 `value`，并附上提示和 `original` 的位置。

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

省略时报告 `"warning"`。也可设为 `"error"`、`"info"` 或 `"hint"`。

### `code`

规则名。省略时不附规则名，提供时不能是空字符串。

### `at`

省略或传 `auto` 时指向调用处。传入 Typst 内容时，指向其源码位置。传 `none` 时省略位置。

### `hints`

给站点作者看的提示，例如修复建议。可省略。

### `related`

相关位置的列表。每项填 `message`（字符串）和 `at`（Typst 内容或 `none`）。
